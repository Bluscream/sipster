//! Telephony: control commands, tray requests, call events, and the four
//! things a user can do to a call.

use super::{
    call_status, chrono_now_iso, dialable, display_name, registration_status, ActiveCall,
    BlockAction, CallEvent, CallId, CallRecord, CallState, CallType, Command, IncomingCall,
    Message, RecordSource, SipsterApp, Task,
};
use crate::{sound, tray};
use iced::window;

impl SipsterApp {
    /// Routes a control command, deferring it if it needs an engine we do not
    /// have yet.
    ///
    /// Only telephony commands need one. Show and Quit must run immediately:
    /// on first run there is no engine, and parking a Quit made an
    /// unconfigured instance impossible to stop with `sipster --quit`.
    pub(super) fn on_ipc(&mut self, cmd: Command) -> Task<Message> {
        let needs_engine = !matches!(
            cmd,
            Command::Show
                | Command::Dial { .. }
                | Command::OpenSettings
                | Command::OpenContacts
                | Command::OpenCallList
                | Command::Quit
        );
        if needs_engine && self.engine.is_none() {
            self.pending_command = Some(cmd);
            return Task::none();
        }
        self.handle_ipc(cmd)
    }

    pub(super) fn handle_ipc(&mut self, cmd: Command) -> Task<Message> {
        match cmd {
            Command::Call { target } => {
                self.dial_number = target;
                let dial_task = self.dial();
                if let Some(id) = self.main_window {
                    Task::batch([window::gain_focus(id), dial_task])
                } else {
                    dial_task
                }
            }
            Command::Dial { target } => {
                self.dial_number = target;
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    Task::none()
                }
            }
            Command::Answer => self.answer(),
            Command::Hangup => {
                if self.incoming.is_some() {
                    self.decline()
                } else {
                    self.hangup()
                }
            }
            Command::Show => {
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    let (id, open) = window::open(crate::main_window_settings());
                    self.main_window = Some(id);
                    Task::batch([open.map(Message::MainOpened), window::gain_focus(id)])
                }
            }
            Command::OpenSettings => self.open_settings(),
            // An explicit "open" from outside the app means a window,
            // whatever the button had cycled to.
            Command::OpenContacts => self.show_contacts_window(),
            Command::OpenCallList => self.show_calls_window(),
            Command::Quit => {
                // A pending Google sign-in runs on a blocking thread, and
                // tokio waits for those at shutdown; without this the process
                // outlives the quit by up to three minutes.
                sipster_integrations::cancel_pending_auth();
                iced::exit()
            }
        }
    }

    pub(super) fn handle_tray(&mut self, req: tray::Request) -> Task<Message> {
        match req {
            tray::Request::Show => {
                if let Some(id) = self.main_window {
                    window::gain_focus(id)
                } else {
                    let (id, open) = window::open(crate::main_window_settings());
                    self.main_window = Some(id);
                    Task::batch([open.map(Message::MainOpened), window::gain_focus(id)])
                }
            }
            tray::Request::OpenSettings => self.open_settings(),
            tray::Request::OpenCallList => self.show_calls_window(),
            tray::Request::OpenContacts => self.show_contacts_window(),
            tray::Request::Answer => self.answer(),
            tray::Request::Hangup => {
                if self.incoming.is_some() {
                    self.decline()
                } else {
                    self.hangup()
                }
            }
            tray::Request::Quit => {
                sipster_integrations::cancel_pending_auth();
                iced::exit()
            }
        }
    }

    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    pub(super) fn on_call_event(&mut self, event: CallEvent) -> Task<Message> {
        match event {
            CallEvent::Registration(state) => {
                self.status = registration_status(&state);
                self.registration = state;
            }
            CallEvent::IncomingCall { id, remote_uri, .. } => {
                // Check if the remote party is blocked
                // Match on the caller's number, not the whole URI. A raw
                // `contains` matched the host and any longer number, and a
                // blank entry matched everything.
                let blocked_entry = self
                    .config
                    .integration
                    .blocked_numbers
                    .iter()
                    .find(|b| sipster_integrations::number_matches(&remote_uri, &b.number));

                if let Some(blocked) = blocked_entry {
                    match blocked.action {
                        BlockAction::Reject => {
                            tracing::info!(remote = %remote_uri, "rejecting call from blocked number");
                            let engine = self.engine.clone();
                            self.status = format!("Rejected blocked call from {remote_uri}");
                            if self.config.integration.local_history_enabled {
                                self.sync_manager.record_local_call(CallRecord {
                                    id: format!("local-blocked-{id}"),
                                    call_type: CallType::Rejected,
                                    remote_number: dialable(&remote_uri),
                                    remote_name: blocked.name.clone(),
                                    local_party: self.account_info.clone(),
                                    timestamp: chrono_now_iso(),
                                    duration_seconds: 0,
                                    source: RecordSource::Local,
                                });
                            }
                            if let Some(eng) = engine {
                                return Task::future(async move {
                                    Message::ActionDone(eng.hangup(id).await.map_err(|e| e.to_string()))
                                });
                            }
                            return Task::none();
                        }
                        BlockAction::Mute => {
                            tracing::info!(remote = %remote_uri, "muting call from blocked number");
                            // Silent: no notification, no ringtone. Still
                            // recorded — a muted call the user never heard is
                            // exactly the one they want to find afterwards.
                            if self.config.integration.local_history_enabled {
                                self.sync_manager.record_local_call(CallRecord {
                                    id: format!("local-muted-{id}"),
                                    call_type: CallType::Incoming,
                                    remote_number: dialable(&remote_uri),
                                    remote_name: blocked.name.clone(),
                                    local_party: self.account_info.clone(),
                                    timestamp: chrono_now_iso(),
                                    duration_seconds: 0,
                                    source: RecordSource::Local,
                                });
                            }
                            self.incoming = Some(IncomingCall { id, remote: remote_uri });
                            self.status = "Incoming call (muted)…".into();
                            return Task::none();
                        }
                    }
                }

                if self.config.ui.notifications {
                    sound::notify_incoming(&remote_uri);
                }
                // Record incoming call in local history if enabled
                if self.config.integration.local_history_enabled {
                    self.sync_manager.record_local_call(CallRecord {
                        id: format!("local-in-{id}"),
                        call_type: CallType::Incoming,
                        remote_number: dialable(&remote_uri),
                        remote_name: display_name(&remote_uri),
                        local_party: self.account_info.clone(),
                        timestamp: chrono_now_iso(),
                        duration_seconds: 0,
                        source: RecordSource::Local,
                    });
                }

                // Assigning drops any previous ringtone, so a second inbound
                // call cannot leave two rings overlapping.
                self.ringtone = self.config.ui.ringtone.then(sound::start_ringing);
                self.incoming = Some(IncomingCall { id, remote: remote_uri });
                self.status = "Incoming call…".into();
            }
            CallEvent::StateChanged { id, state } => {
                self.apply_state(id, state);
            }
            CallEvent::Terminated { id, reason } => {
                if let Some(active) = self.active.take().filter(|c| c.id == id) {
                    self.chime(sound::call_ended);
                    if self.config.integration.local_history_enabled {
                        self.sync_manager.record_local_call(CallRecord {
                            id: format!("local-term-{id}"),
                            call_type: CallType::Outgoing,
                            remote_number: dialable(&active.remote),
                            remote_name: None,
                            local_party: self.account_info.clone(),
                            timestamp: chrono_now_iso(),
                            duration_seconds: 0,
                            source: RecordSource::Local,
                        });
                    }
                }
                if let Some(incoming) = self.incoming.take().filter(|c| c.id == id) {
                    self.ringtone = None;
                    if self.config.integration.local_history_enabled {
                        self.sync_manager.record_local_call(CallRecord {
                            id: format!("local-missed-{id}"),
                            call_type: CallType::Missed,
                            remote_number: dialable(&incoming.remote),
                            remote_name: None,
                            local_party: self.account_info.clone(),
                            timestamp: chrono_now_iso(),
                            duration_seconds: 0,
                            source: RecordSource::Local,
                        });
                    }
                }
                self.status = format!("Call ended: {reason}");
            }
        }
        self.sync_tray_state();
        Task::none()
    }
    /// Plays a call chime, unless the user turned chimes off.
    pub(super) fn chime(&self, play: fn()) {
        if self.config.ui.call_chimes {
            play();
        }
    }

    pub(super) fn sync_tray_state(&self) {
        let Some(tray) = &self.tray else { return };
        let state = if self.incoming.is_some() {
            tray::CallState::Ringing
        } else if self.active.is_some() {
            tray::CallState::InCall
        } else {
            tray::CallState::Idle
        };
        tray.set_call_state(state);
    }
    pub(super) fn apply_state(&mut self, id: CallId, state: CallState) {
        let remote = self
            .active
            .as_ref()
            .filter(|c| c.id == id)
            .map(|c| c.remote.clone())
            .or_else(|| self.incoming.as_ref().filter(|c| c.id == id).map(|c| c.remote.clone()))
            .unwrap_or_else(|| self.dial_number.clone());
        self.active = Some(ActiveCall { id, state, remote });
        self.status = call_status(state);
    }

    pub(super) fn dial(&mut self) -> Task<Message> {
        let (Some(engine), false) = (&self.engine, self.dial_number.is_empty()) else {
            return Task::none();
        };
        self.chime(sound::call_started);
        let engine = engine.clone();
        let target = self.dial_number.clone();

        if self.config.integration.local_history_enabled {
            self.sync_manager.record_local_call(CallRecord {
                id: format!("local-out-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()),
                call_type: CallType::Outgoing,
                remote_number: target.clone(),
                remote_name: None,
                local_party: self.account_info.clone(),
                timestamp: chrono_now_iso(),
                duration_seconds: 0,
                source: RecordSource::Local,
            });
        }

        self.status = format!("Dialing {target}…");
        Task::future(async move { Message::Dialed(engine.dial(&target).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn hangup(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.active.take()) else {
            return Task::none();
        };
        self.chime(sound::call_ended);
        let engine = engine.clone();
        let id = call.id;
        self.status = "Hanging up…".into();
        self.sync_tray_state();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn answer(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.incoming.take()) else {
            return Task::none();
        };
        self.ringtone = None;
        let (engine, id) = (engine.clone(), call.id);
        self.status = "Answering call…".into();
        self.active = Some(ActiveCall {
            id,
            state: CallState::Active,
            remote: call.remote,
        });
        Task::future(async move { Message::ActionDone(engine.answer(id).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn decline(&mut self) -> Task<Message> {
        let (Some(engine), Some(call)) = (&self.engine, self.incoming.take()) else {
            return Task::none();
        };
        self.ringtone = None;
        let (engine, id) = (engine.clone(), call.id);
        self.status = "Call declined".into();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn on_dial_input_changed(&mut self, input: String) {
        if self.config.ui.dtmf_feedback && input.len() > self.dial_number.len() {
            if let Some(ch) = input.chars().last() {
                sound::dtmf(ch);
            }
        }
        self.dial_number = input;
    }
}
