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
fn email_address_requires_a_local_part_a_domain_and_no_whitespace() {
    assert!(email_address("user@example.org").is_ok());
    for bad in ["", "no-at-sign", "@example.org", "user@", "user@nodot", "us er@example.org"] {
        assert_eq!(email_address(bad), Err("invalid_params"));
    }
}

#[test]
fn attachments_are_capped_in_count_and_combined_size() {
    assert_eq!(attachments(&json!({})).unwrap(), Vec::<Value>::new());
    let one = json!({"attachments": [{"filename":"a.txt","contentType":"text/plain","content":"aGVsbG8="}]});
    assert_eq!(
        attachments(&one).unwrap(),
        vec![json!({"filename":"a.txt","content_type":"text/plain","content":"aGVsbG8="})]
    );
    let too_many: Vec<Value> = (0..MAX_ATTACHMENTS + 1)
        .map(|_| json!({"filename":"a","contentType":"text/plain","content":"x"}))
        .collect();
    assert_eq!(
        attachments(&json!({"attachments": too_many})),
        Err("invalid_params")
    );
    let oversized = json!({"attachments": [{
        "filename":"a","contentType":"text/plain",
        "content": "x".repeat(MAX_ATTACHMENT_TOTAL + 1)
    }]});
    assert_eq!(attachments(&oversized), Err("invalid_params"));
}

#[test]
fn optional_turns_an_absent_value_into_json_null_never_an_empty_string() {
    assert_eq!(optional(""), Value::Null);
    assert_eq!(optional("x"), json!("x"));
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
        ("lnemail.read", json!({"accountId":"not-an-account","id":"abc"})),
        ("lnemail.read", json!({"accountId":"lnemail:user@example.org","id":".."})),
        ("lnemail.delete", json!({"accountId":"not-an-account","id":"abc"})),
        ("lnemail.deleteMany", json!({"accountId":"not-an-account","ids":["abc"]})),
        ("lnemail.deleteMany", json!({"accountId":"lnemail:user@example.org","ids":[]})),
        ("lnemail.recentSends", json!({"accountId":"not-an-account"})),
        (
            "lnemail.send",
            json!({"accountId":"not-an-account","recipient":"user@example.org","subject":"","body":""}),
        ),
        (
            "lnemail.send",
            json!({"accountId":"lnemail:user@example.org","recipient":"bad","subject":"","body":""}),
        ),
        ("lnemail.sendStatus", json!({"accountId":"not-an-account","paymentHash":"abc"})),
        ("lnemail.sendInvoice", json!({"accountId":"not-an-account","paymentHash":"abc"})),
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
