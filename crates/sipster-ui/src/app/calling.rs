//! Telephony: control commands, tray requests, call events, and the four
//! things a user can do to a call.

use super::{
    call_status, chrono_now_iso, dialable, display_name, registration_status, run_hook,
    ActiveCall, BlockAction, CallEvent, CallId, CallRecord, CallState, CallType, Command,
    IncomingCall, Message, RecordSource, RegistrationState, SipsterApp, Task,
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
        if needs_engine && self.engine().is_none() {
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
            Command::Show => self.show_main_window(),
            Command::SetHold { hold } => {
                // Only meaningful mid-call; ignored otherwise rather than
                // reported as an error, the same as Answer with nothing
                // ringing.
                if self.active.as_ref().is_some_and(|c| c.on_hold != hold) {
                    return self.toggle_hold();
                }
                Task::none()
            }
            Command::Transfer { target } => {
                self.dial_number = target;
                self.transfer()
            }
            Command::Dtmf { digit } => self.send_dtmf(digit).unwrap_or_else(Task::none),
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

    /// Brings the dialer back, from the tray or a `sipster://show` command.
    ///
    /// "Show" covers three states, and only one of them is a plain raise:
    ///
    /// - **Closed to tray.** There is no window, so one is opened. The focus
    ///   request is chained rather than batched, because it means nothing
    ///   until the window exists.
    /// - **Buried behind other windows.** Asking for focus is all a client
    ///   can do; whether it is granted is the compositor's call.
    /// - **Minimized.** This one cannot be fixed by asking. `xdg_toplevel` has
    ///   a `set_minimized` request and no matching unset, so under Wayland
    ///   *no* client can restore its own window — `gain_focus` and
    ///   `minimize(false)` are both silently no-ops, which is exactly what
    ///   "clicking the tray does nothing" looked like. Nor can it be detected:
    ///   `is_minimized` comes back `None` for the same reason. So focus is
    ///   requested and then checked, and [`SipsterApp::on_show_fallback`]
    ///   rebuilds the window when the request went nowhere.
    pub(super) fn show_main_window(&mut self) -> Task<Message> {
        let Some(id) = self.main_window else {
            let (id, open) = window::open(crate::main_window_settings());
            self.main_window = Some(id);
            return open.map(Message::MainOpened).chain(window::gain_focus(id));
        };

        // Ask nicely first, then check whether it worked: `is_minimized`
        // cannot be used to decide up front, because Wayland has no way to
        // report it either and it comes back `None`.
        Task::batch([
            window::gain_focus(id),
            Task::future(async move {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                Message::ShowFallback(id)
            }),
        ])
    }

    /// Rebuilds the dialer when asking for focus did not bring it back.
    ///
    /// A window that is merely behind another usually takes focus, so this
    /// does nothing. A minimized one cannot — see
    /// [`SipsterApp::show_main_window`] — and a replacement is the only way to
    /// get it on screen again.
    ///
    /// The old window is closed *first* and the new one opened from the close
    /// event. Opening the replacement while the original was still up killed
    /// the Wayland event loop outright ("error dispatching event loop"), which
    /// took the whole app with it. `main_window` is cleared here so the close
    /// is not mistaken for the user closing the dialer, which would otherwise
    /// quit with close-to-tray switched off.
    pub(super) fn on_show_fallback(&mut self, id: window::Id) -> Task<Message> {
        if self.main_window != Some(id) || self.main_focused {
            return Task::none();
        }
        tracing::info!("focus request did not land; rebuilding the dialer window");
        self.main_window = None;
        self.reopening_main = true;
        window::close(id)
    }

    pub(super) fn handle_tray(&mut self, req: tray::Request) -> Task<Message> {
        tracing::debug!(?req, "tray request");
        match req {
            tray::Request::Show => self.show_main_window(),
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
                match &state {
                    RegistrationState::Registered => {
                        if let Some(ref cmd) = self.config.commands.on_sip_registered {
                            let _ = run_hook(
                                cmd,
                                &[
                                    ("user", &self.config.account.username),
                                    ("registrar", &self.config.account.registrar),
                                    ("port", &self.config.account.port.to_string()),
                                ],
                            );
                        }
                    }
                    RegistrationState::Unregistered => {
                        if let Some(ref cmd) = self.config.commands.on_sip_unregistered {
                            let _ = run_hook(
                                cmd,
                                &[
                                    ("user", &self.config.account.username),
                                    ("registrar", &self.config.account.registrar),
                                    ("port", &self.config.account.port.to_string()),
                                ],
                            );
                        }
                    }
                    RegistrationState::Failed(err) => {
                        if let Some(ref cmd) = self.config.commands.on_sip_registration_failed {
                            let _ = run_hook(cmd, &[("error", err.as_str())]);
                        }
                    }
                    RegistrationState::Registering => {}
                }
                self.status = registration_status(&state);
                self.registration = state;
            }
            CallEvent::IncomingCall { id, remote_uri, .. } => {
                if let Some(ref cmd) = self.config.commands.on_call_incoming {
                    let _ = run_hook(
                        cmd,
                        &[
                            ("number", &dialable(&remote_uri)),
                            ("name", &display_name(&remote_uri).unwrap_or_default()),
                        ],
                    );
                }

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
                            let engine = self.engine().cloned();
                            self.status = rust_i18n::t!("call.rejected_blocked", number = remote_uri).to_string();
                            if self.config.integration.local_history_enabled {
                                self.sync_manager.record_local_call(CallRecord {
                                    id: format!("local-blocked-{id}"),
                                    call_type: CallType::Rejected,
                                    remote_number: dialable(&remote_uri),
                                    remote_name: blocked.name.clone(),
                                    local_party: self.local_party(),
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
                                    local_party: self.local_party(),
                                    timestamp: chrono_now_iso(),
                                    duration_seconds: 0,
                                    source: RecordSource::Local,
                                });
                            }
                            self.incoming = Some(IncomingCall { id, remote: remote_uri });
                            self.status = rust_i18n::t!("call.incoming_muted").to_string();
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
                        local_party: self.local_party(),
                        timestamp: chrono_now_iso(),
                        duration_seconds: 0,
                        source: RecordSource::Local,
                    });
                }

                // Assigning drops any previous ringtone, so a second inbound
                // call cannot leave two rings overlapping.
                self.ringtone = self.config.ui.ringtone.then(sound::start_ringing);
                self.incoming = Some(IncomingCall { id, remote: remote_uri });
                self.status = rust_i18n::t!("call.incoming_ellipsis").to_string();
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
                            local_party: self.local_party(),
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
                            local_party: self.local_party(),
                            timestamp: chrono_now_iso(),
                            duration_seconds: 0,
                            source: RecordSource::Local,
                        });
                    }
                }
                if let Some(ref cmd) = self.config.commands.on_call_ended {
                    let _ = run_hook(cmd, &[("reason", reason.as_str())]);
                }
                self.status = rust_i18n::t!("call.call_ended_reason", reason = reason.clone()).to_string();
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
        let on_hold = self.active.as_ref().is_some_and(|c| c.id == id && c.on_hold);

        if state == CallState::Active {
            if let Some(ref cmd) = self.config.commands.on_call_connected {
                let _ = run_hook(
                    cmd,
                    &[
                        ("number", &dialable(&remote)),
                        ("name", &display_name(&remote).unwrap_or_default()),
                    ],
                );
            }
        }

        self.active = Some(ActiveCall { id, state, remote, on_hold });
        self.status = call_status(state);
    }

    pub(super) fn dial(&mut self) -> Task<Message> {
        let (Some(engine), false) = (self.engine(), self.dial_number.is_empty()) else {
            return Task::none();
        };
        self.chime(sound::call_started);
        let engine = engine.clone();
        let target = self.dial_number.clone();

        if let Some(ref cmd) = self.config.commands.on_call_outgoing {
            let _ = run_hook(cmd, &[("number", target.as_str()), ("name", "")]);
        }

        if self.config.integration.local_history_enabled {
            self.sync_manager.record_local_call(CallRecord {
                id: format!("local-out-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()),
                call_type: CallType::Outgoing,
                remote_number: target.clone(),
                remote_name: None,
                local_party: self.local_party(),
                timestamp: chrono_now_iso(),
                duration_seconds: 0,
                source: RecordSource::Local,
            });
        }

        self.status = rust_i18n::t!("call.dialing_target", target = target).to_string();
        Task::future(async move { Message::Dialed(engine.dial(&target).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn hangup(&mut self) -> Task<Message> {
        // The call's own account, not the selected one — they differ as soon
        // as a second line rings while the first is picked.
        let Some(call) = self.active.take() else {
            return Task::none();
        };
        let Some(engine) = self.engine().cloned() else {
            return Task::none();
        };
        self.chime(sound::call_ended);
        let id = call.id;
        self.status = rust_i18n::t!("call.hanging_up").to_string();
        self.sync_tray_state();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn answer(&mut self) -> Task<Message> {
        let Some(call) = self.incoming.take() else {
            return Task::none();
        };
        let Some(engine) = self.engine().cloned() else {
            return Task::none();
        };
        self.ringtone = None;
        let id = call.id;
        self.status = rust_i18n::t!("call.answering_call").to_string();
        self.active = Some(ActiveCall {
            id,
            state: CallState::Active,
            remote: call.remote,
            on_hold: false,
        });
        Task::future(async move { Message::ActionDone(engine.answer(id).await.map_err(|e| e.to_string())) })
    }

    pub(super) fn decline(&mut self) -> Task<Message> {
        let Some(call) = self.incoming.take() else {
            return Task::none();
        };
        let Some(engine) = self.engine().cloned() else {
            return Task::none();
        };
        self.ringtone = None;
        let id = call.id;
        self.status = rust_i18n::t!("call.call_declined").to_string();
        Task::future(async move { Message::ActionDone(engine.hangup(id).await.map_err(|e| e.to_string())) })
    }

    /// Records a hold or resume the far end accepted.
    pub(super) fn on_hold_changed(&mut self, on_hold: bool) -> Task<Message> {
        let (number, name) = if let Some(call) = self.active.as_mut() {
            call.on_hold = on_hold;
            (dialable(&call.remote), display_name(&call.remote).unwrap_or_default())
        } else {
            (String::new(), String::new())
        };

        if on_hold {
            if let Some(ref cmd) = self.config.commands.on_call_held {
                let _ = run_hook(cmd, &[("number", number.as_str()), ("name", name.as_str())]);
            }
        } else if let Some(ref cmd) = self.config.commands.on_call_unheld {
            let _ = run_hook(cmd, &[("number", number.as_str()), ("name", name.as_str())]);
        }

        self.status = if on_hold {
            rust_i18n::t!("call.on_hold").to_string()
        } else {
            rust_i18n::t!("call.connected").to_string()
        };
        Task::none()
    }

    /// Puts the call on hold, or takes it off again.
    ///
    /// The flag is only flipped once the far end accepts, so the button never
    /// claims a hold that did not happen.
    pub(super) fn toggle_hold(&mut self) -> Task<Message> {
        let Some(call) = self.active.as_ref() else {
            return Task::none();
        };
        let Some(engine) = self.engine().cloned() else {
            return Task::none();
        };
        let (id, hold) = (call.id, !call.on_hold);
        self.status = if hold {
            rust_i18n::t!("call.holding_ellipsis").to_string()
        } else {
            rust_i18n::t!("call.resuming_ellipsis").to_string()
        };
        Task::future(async move {
            match engine.set_hold(id, hold).await {
                Ok(()) => Message::HoldChanged(hold),
                Err(e) => Message::ActionDone(Err(e.to_string())),
            }
        })
    }

    /// Hands the call to whatever is in the dial field and drops out of it.
    pub(super) fn transfer(&mut self) -> Task<Message> {
        let target = self.dial_number.trim().to_string();
        let Some(call) = self.active.as_ref() else {
            return Task::none();
        };
        let Some(engine) = self.engine().cloned() else {
            return Task::none();
        };
        if target.is_empty() {
            self.status = rust_i18n::t!("call.type_number_transfer").to_string();
            return Task::none();
        }
        let id = call.id;
        self.status = rust_i18n::t!("call.transferring_to", target = target).to_string();
        Task::future(async move {
            Message::ActionDone(
                engine
                    .transfer_blind(id, &target)
                    .await
                    .map_err(|e| e.to_string()),
            )
        })
    }

    /// Sends `digit` to the far end when a call is up.
    ///
    /// `None` when there is no call to send it to, which is the caller's cue
    /// to treat the keypress as editing the number instead.
    pub(super) fn send_dtmf(&self, digit: char) -> Option<Task<Message>> {
        let call = self.active.as_ref()?;
        if !sipster_core::engine::is_dtmf_digit(digit) {
            return None;
        }
        let engine = self.engine()?.clone();
        let id = call.id;
        Some(Task::future(async move {
            Message::ActionDone(engine.send_dtmf(id, digit).await.map_err(|e| e.to_string()))
        }))
    }

    pub(super) fn on_dial_input_changed(&mut self, input: String) -> Task<Message> {
        // Typed rather than clicked: light the matching pad key so the two
        // halves of the dialer read as one control. A shortened field is a
        // backspace, which lights the ⌫ key instead.
        if input.len() > self.dial_number.len() {
            if let Some(ch) = input.chars().last() {
                if self.config.ui.dtmf_feedback {
                    sound::dtmf(ch);
                }
                self.glow.strike(ch);
                // Typed during a call: send the tone and leave the field
                // alone, so it keeps showing the number that was dialled.
                if let Some(task) = self.send_dtmf(ch) {
                    return task;
                }
            }
        } else if input.len() < self.dial_number.len() {
            self.glow.strike('⌫');
        }
        self.dial_number = input;
        Task::none()
    }
}
