use sipster_core::{CallEvent, SipAccount, SipClient};

#[tokio::test]
async fn test_sip_account_default_config() {
    let account = SipAccount::default();
    assert_eq!(account.port, 5060);
    assert_eq!(account.label, "Default Account");
    assert!(account.registrar.is_empty());
}

#[tokio::test]
async fn test_client_event_dispatch() {
    let account = SipAccount {
        label: "Test PBX".into(),
        registrar: "127.0.0.1".into(),
        port: 5060,
        username: "100".into(),
        auth_user: "100".into(),
        password: "secret".into(),
    };

    let client = SipClient::new(account).expect("Failed to create client");
    let mut rx = client.subscribe_events();

    client.register().await.expect("Registration failed");

    match rx.recv().await {
        Ok(CallEvent::RegistrationSuccess) => {}
        other => panic!("Expected RegistrationSuccess, got {other:?}"),
    }

    let call_id = client.dial("101").await.expect("Dial failed");
    match rx.recv().await {
        Ok(CallEvent::Ringing { id }) => assert_eq!(id, call_id),
        other => panic!("Expected Ringing, got {other:?}"),
    }

    client.hangup(call_id).await.expect("Hangup failed");
    match rx.recv().await {
        Ok(CallEvent::Terminated { id, reason }) => {
            assert_eq!(id, call_id);
            assert!(reason.contains("ended by user"));
        }
        other => panic!("Expected Terminated, got {other:?}"),
    }
}
