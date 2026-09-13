use super::*;

#[test]
fn allowed_refuses_any_key_outside_the_method_s_own_list() {
    assert!(allowed(&json!({"accountId":"x"}), &["accountId"]).is_ok());
    assert_eq!(
        allowed(&json!({"accountId":"x","extra":"y"}), &["accountId"]),
        Err("invalid_params")
    );
    assert_eq!(allowed(&json!("not an object"), &["accountId"]), Err("invalid_params"));
}

#[test]
fn a_path_segment_is_never_a_slash_a_dot_or_control_bytes() {
    assert_eq!(segment(&json!({"id":"abc-123_DEF"}), "id").unwrap(), "abc-123_DEF");
    for bad in ["", "a/b", "..", ".", "a b", "a\nb", "a\0b", &"a".repeat(129)] {
        assert_eq!(segment(&json!({"id": bad}), "id"), Err("invalid_params"));
    }
    assert_eq!(segment(&json!({}), "id"), Err("invalid_params"));
    assert_eq!(segment(&json!({"id": 5}), "id"), Err("invalid_params"));
}

#[test]
fn text_enforces_the_caller_s_bound_and_refuses_nul() {
    assert_eq!(text(&json!({"s":"hello"}), "s", true, 10).unwrap(), "hello");
    assert_eq!(text(&json!({}), "s", false, 10).unwrap(), "");
    assert_eq!(text(&json!({}), "s", true, 10), Err("invalid_params"));
    assert_eq!(text(&json!({"s":"toolong"}), "s", true, 3), Err("invalid_params"));
    assert_eq!(text(&json!({"s":"a\0b"}), "s", true, 10), Err("invalid_params"));
    // A body may carry a newline: only NUL is refused here.
    assert_eq!(text(&json!({"s":"line one\nline two"}), "s", true, 100).unwrap(), "line one\nline two");
}

#[test]
fn boolean_defaults_to_false_and_refuses_anything_but_a_bool() {
    assert!(!boolean(&json!({}), "b").unwrap());
    assert!(boolean(&json!({"b":true}), "b").unwrap());
    assert_eq!(boolean(&json!({"b":"true"}), "b"), Err("invalid_params"));
}

#[test]
fn optional_turns_an_absent_value_into_json_null_never_an_empty_string() {
    assert_eq!(optional(""), Value::Null);
    assert_eq!(optional("x"), json!("x"));
}

#[tokio::test]
async fn an_explicit_token_is_used_directly_and_never_needs_an_account() {
    // Verifying a pasted token, or one a signup invoice just paid for, happens
    // before the account is registered — so this must not touch the keyring.
    assert_eq!(token(&json!({"token": "sekret"})).await.unwrap(), "sekret");
    assert_eq!(token(&json!({})).await, Err("invalid_params"));
    for bad in ["", "a\nb", "a\0b"] {
        assert_eq!(token(&json!({"token": bad})).await, Err("invalid_params"));
    }
}

#[tokio::test]
async fn every_method_validates_its_params_before_any_credential_lookup() {
    // A malformed accountId is refused by auth::password before it would ever
    // shell out to the keyring, so these exercise the full `call` dispatch
    // without touching the network or a running secret service.
    for (method, params) in [
        ("lnemail.createAccount", json!({"unexpected":true})),
        ("lnemail.signupInvoice", json!({"paymentHash":"a/b"})),
        ("lnemail.paymentStatus", json!({"paymentHash":""})),
        ("lnemail.account", json!({"accountId":"not-an-account"})),
        ("lnemail.list", json!({"accountId":"not-an-account"})),
        ("lnemail.list", json!({"accountId":"not-an-account","query":"sent"})),
        ("lnemail.read", json!({"accountId":"not-an-account","id":"abc"})),
        ("lnemail.read", json!({"accountId":"lnemail:user@example.org","id":".."})),
        ("lnemail.attachment", json!({"accountId":"not-an-account","id":"abc","attachmentId":"0"})),
        ("lnemail.attachment", json!({"accountId":"lnemail:user@example.org","id":"abc","attachmentId":"not-a-number"})),
        ("lnemail.delete", json!({"accountId":"not-an-account","id":"abc"})),
        ("lnemail.deleteMany", json!({"accountId":"not-an-account","ids":["abc"]})),
        ("lnemail.deleteMany", json!({"accountId":"lnemail:user@example.org","ids":[]})),
        ("lnemail.recentSends", json!({"accountId":"not-an-account"})),
        ("lnemail.send", json!({"accountId":"not-an-account","raw":"not valid base64!!"})),
        (
            "lnemail.send",
            // Valid base64url of a message with no recipient at all: refused
            // by `decode_outgoing` itself, still before any keyring lookup.
            json!({"accountId":"lnemail:user@example.org","raw": URL_SAFE_NO_PAD.encode("Subject: hi\r\n\r\nbody")}),
        ),
        ("lnemail.sendStatus", json!({"accountId":"not-an-account","paymentHash":"abc"})),
        ("lnemail.sendInvoice", json!({"accountId":"not-an-account","paymentHash":"abc"})),
        ("lnemail.renewalInvoice", json!({"accountId":"not-an-account"})),
        ("lnemail.renewalInvoice", json!({"accountId":"lnemail:user@example.org","years":0})),
        ("lnemail.renewalInvoice", json!({"accountId":"lnemail:user@example.org","years":11})),
        ("lnemail.renewalInvoice", json!({"accountId":"lnemail:user@example.org","years":1.5})),
        ("lnemail.renewalStatus", json!({"accountId":"not-an-account","paymentHash":"abc"})),
        ("lnemail.renewalReissue", json!({"accountId":"not-an-account","paymentHash":"abc"})),
        ("lnemail.unknown", json!({})),
    ] {
        assert!(
            call(method, &params).await.is_err(),
            "{method} accepted {params}"
        );
    }
}

#[tokio::test]
async fn unknown_method_is_refused() {
    assert_eq!(call("lnemail.bogus", &json!({})).await, Err("unknown_method"));
}

#[test]
fn sent_hash_recognizes_only_its_own_prefix() {
    assert_eq!(sent_hash("send-abc123"), Some("abc123"));
    assert_eq!(sent_hash("abc123"), None);
    assert_eq!(sent_hash("send-"), Some(""));
    assert_eq!(sent_hash("sender-abc"), None);
}

#[tokio::test]
async fn a_sent_log_row_is_refused_deletion_before_any_network_call() {
    // The guard runs right after segment() validates the id shape, so a
    // clearly-valid accountId format proves this is the sent-log check and
    // not an unrelated parameter failure.
    let account = json!("lnemail:user@example.org");
    assert_eq!(
        call("lnemail.delete", &json!({"accountId": account, "id": "send-abc123"})).await,
        Err("lnemail_sent_log_readonly")
    );
    assert_eq!(
        call(
            "lnemail.deleteMany",
            &json!({"accountId": account, "ids": ["send-abc123"]})
        )
        .await,
        Err("lnemail_sent_log_readonly")
    );
    assert_eq!(
        call(
            "lnemail.attachment",
            &json!({"accountId": account, "id": "send-abc123", "attachmentId": "0"})
        )
        .await,
        Err("lnemail_not_found")
    );
}
