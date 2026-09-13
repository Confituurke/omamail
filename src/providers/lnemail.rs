//! LNemail: a disposable mailbox paid for over the Lightning Network, reached
//! over its own bearer-token REST API rather than IMAP, JMAP or a vendor SDK.
//!
//! Three calls need no account and no token at all — creating a mailbox and
//! checking whether its signup invoice has been paid happen before one
//! exists. Every other call is bound to a registered account and its stored
//! bearer token, which is the mailbox's only credential: LNemail issues no
//! refresh token and rotates nothing, so there is no session state to keep
//! here beyond the HTTP transport itself.
use super::lnemail_http as http;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

#[path = "lnemail_resource.rs"]
mod resource;

const MAX_DELETE_BATCH: usize = 500;
// The same decoded-message ceiling `imap.send` accepts, since this is the
// same `raw` field every provider's send is handed.
const MAX_RAW_MESSAGE_DECODED: usize = 32 * 1024 * 1024;
const MAX_RAW_MESSAGE_ENCODED: usize = MAX_RAW_MESSAGE_DECODED * 4 / 3 + 4;

fn allowed(p: &Value, keys: &[&str]) -> Result<(), &'static str> {
    if p.as_object()
        .ok_or("invalid_params")?
        .keys()
        .any(|k| !keys.contains(&k.as_str()))
    {
        return Err("invalid_params");
    }
    Ok(())
}

/// A path component: an id or a payment hash, never a slash or a URL of its own.
fn segment<'a>(p: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    let value = p.get(key).and_then(Value::as_str).ok_or("invalid_params")?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid_params");
    }
    Ok(value)
}

/// A row in the recent-sends log names itself by its payment hash, prefixed
/// so it can never collide with a real message id: LNemail's own ids and its
/// payment hashes are separate namespaces, but nothing declares that in
/// writing, and this is one byte string doing the declaring instead.
const SENT_ID_PREFIX: &str = "send-";

fn sent_hash(id: &str) -> Option<&str> {
    id.strip_prefix(SENT_ID_PREFIX)
}

fn text<'a>(p: &'a Value, key: &str, required: bool, max: usize) -> Result<&'a str, &'static str> {
    match p.get(key) {
        Some(Value::String(s)) => {
            if s.len() > max || s.bytes().any(|b| b == 0) {
                return Err("invalid_params");
            }
            Ok(s.as_str())
        }
        None | Some(Value::Null) if !required => Ok(""),
        _ => Err("invalid_params"),
    }
}

fn boolean(p: &Value, key: &str) -> Result<bool, &'static str> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err("invalid_params"),
    }
}

fn optional(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        json!(value)
    }
}

/// A token supplied directly, for verifying a pasted or freshly paid-for
/// token before the account exists to resolve one through the keyring.
fn explicit_token(p: &Value) -> Result<Option<&str>, &'static str> {
    match p.get("token") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            if s.is_empty() || s.len() > 16384 || s.bytes().any(|b| b < 32 || b == 127) {
                return Err("invalid_params");
            }
            Ok(Some(s.as_str()))
        }
        _ => Err("invalid_params"),
    }
}

async fn token(p: &Value) -> Result<String, &'static str> {
    let account = p.get("accountId").and_then(Value::as_str).ok_or("invalid_params")?;
    crate::auth::password("lnemail", account).await
}

/// The bearer for `GET /account`, the one call that may run before an
/// account is registered: a token just pasted or paid for, or — once
/// registered — the keyring's own. Deliberately its own function rather
/// than a branch inside `token`: only this call site ever reaches
/// `explicit_token`, so a caller-supplied token can authorize nothing but
/// asking LNemail whose it is, by construction rather than by every other
/// match arm's allowlist happening to leave "token" out.
async fn account_token(p: &Value) -> Result<String, &'static str> {
    if let Some(explicit) = explicit_token(p)? {
        return Ok(explicit.to_owned());
    }
    token(p).await
}

pub async fn call(method: &str, p: &Value) -> Result<Value, &'static str> {
    match method {
        "lnemail.createAccount" => {
            allowed(p, &["includeEmail", "includeToken"])?;
            let body = json!({
                "include_email": boolean(p, "includeEmail")?,
                "include_token": boolean(p, "includeToken")?,
            });
            http::post(&["email"], &body, "").await
        }
        "lnemail.signupInvoice" => {
            allowed(p, &["paymentHash", "excludeProvider"])?;
            let hash = segment(p, "paymentHash")?;
            let exclude = text(p, "excludeProvider", false, 256)?;
            let body = json!({"exclude_provider": optional(exclude)});
            http::post(&["email", hash, "new-invoice"], &body, "").await
        }
        "lnemail.paymentStatus" => {
            allowed(p, &["paymentHash"])?;
            let hash = segment(p, "paymentHash")?;
            http::get(&["payment", hash], "").await
        }
        "lnemail.account" => {
            allowed(p, &["accountId", "token"])?;
            let bearer = account_token(p).await?;
            http::get(&["account"], &bearer).await
        }
        "lnemail.list" => {
            allowed(p, &["accountId", "query"])?;
            let bearer = token(p).await?;
            if text(p, "query", false, 64)?.trim() == "sent" {
                // LNemail keeps no sent archive, only a short status log of
                // its own recent outgoing payments — never a body.
                let answer = http::get(&["email", "sends", "recent"], &bearer).await?;
                let entries = answer["sends"].as_array().ok_or("lnemail_invalid_response")?;
                let messages: Vec<Value> = entries.iter().map(resource::sent_row).collect();
                let ids: Vec<Value> = messages.iter().map(|m| m["id"].clone()).collect();
                return Ok(json!({
                    "ids": ids, "messages": messages, "threadIds": ids,
                    "nextPageToken": "", "estimate": entries.len(),
                }));
            }
            let answer = http::get(&["emails"], &bearer).await?;
            let entries = answer["emails"].as_array().ok_or("lnemail_invalid_response")?;
            let messages: Vec<Value> = entries.iter().map(resource::list_row).collect();
            let ids: Vec<Value> = messages.iter().map(|m| m["id"].clone()).collect();
            Ok(json!({
                "ids": ids, "messages": messages, "threadIds": ids,
                "nextPageToken": "", "estimate": entries.len(),
            }))
        }
        "lnemail.read" => {
            allowed(p, &["accountId", "id"])?;
            let id = segment(p, "id")?;
            let bearer = token(p).await?;
            if let Some(hash) = sent_hash(id) {
                // No endpoint answers for one send by its hash; the recent
                // list is all there is, so the matching row is pulled out of
                // it — a small, capped list, not a per-item fetch.
                let answer = http::get(&["email", "sends", "recent"], &bearer).await?;
                let entries = answer["sends"].as_array().ok_or("lnemail_invalid_response")?;
                let item = entries
                    .iter()
                    .find(|entry| entry["payment_hash"] == hash)
                    .ok_or("lnemail_not_found")?;
                return Ok(resource::sent_message(item));
            }
            let answer = http::get(&["emails", id], &bearer).await?;
            Ok(resource::full_message(&answer))
        }
        "lnemail.attachment" => {
            allowed(p, &["accountId", "id", "attachmentId"])?;
            let id = segment(p, "id")?;
            if sent_hash(id).is_some() {
                // A sent row carries no attachments of its own to fetch.
                return Err("lnemail_not_found");
            }
            let index: usize = p["attachmentId"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .ok_or("invalid_params")?;
            let bearer = token(p).await?;
            let answer = http::get(&["emails", id], &bearer).await?;
            resource::attachment_data(&answer, index)
        }
        "lnemail.delete" => {
            allowed(p, &["accountId", "id"])?;
            let id = segment(p, "id")?;
            if sent_hash(id).is_some() {
                // There is no endpoint to remove an entry from the send log;
                // it is LNemail's own record of what it was paid to send.
                return Err("lnemail_sent_log_readonly");
            }
            let bearer = token(p).await?;
            http::delete(&["emails", id], None, &bearer).await
        }
        "lnemail.deleteMany" => {
            allowed(p, &["accountId", "ids"])?;
            let ids = p["ids"].as_array().ok_or("invalid_params")?;
            if ids.is_empty() || ids.len() > MAX_DELETE_BATCH {
                return Err("invalid_params");
            }
            let mut checked = Vec::with_capacity(ids.len());
            for (index, _) in ids.iter().enumerate() {
                let wrapped = json!({"id": ids[index]});
                let id = segment(&wrapped, "id")?;
                if sent_hash(id).is_some() {
                    return Err("lnemail_sent_log_readonly");
                }
                checked.push(id.to_owned());
            }
            let bearer = token(p).await?;
            let body = json!({"email_ids": checked});
            http::delete(&["emails"], Some(&body), &bearer).await
        }
        "lnemail.recentSends" => {
            allowed(p, &["accountId"])?;
            let bearer = token(p).await?;
            http::get(&["email", "sends", "recent"], &bearer).await
        }
        "lnemail.send" => {
            // `raw` is the complete RFC 822 message `MailAccount` builds for
            // every provider, attachments already embedded — the same shape
            // Gmail's send endpoint takes and IMAP puts on the wire
            // unchanged. LNemail's own endpoint wants none of that, so it is
            // taken apart into the fields the endpoint has.
            allowed(p, &["accountId", "raw"])?;
            let encoded = text(p, "raw", true, MAX_RAW_MESSAGE_ENCODED)?;
            let bytes = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| "invalid_params")?;
            if bytes.len() > MAX_RAW_MESSAGE_DECODED {
                return Err("invalid_params");
            }
            let fields = resource::decode_outgoing(&bytes)?;
            let bearer = token(p).await?;
            http::post(&["email", "send"], &fields, &bearer).await
        }
        "lnemail.sendStatus" => {
            allowed(p, &["accountId", "paymentHash"])?;
            let hash = segment(p, "paymentHash")?;
            let bearer = token(p).await?;
            http::get(&["email", "send", "status", hash], &bearer).await
        }
        "lnemail.sendInvoice" => {
            allowed(p, &["accountId", "paymentHash", "excludeProvider"])?;
            let hash = segment(p, "paymentHash")?;
            let exclude = text(p, "excludeProvider", false, 256)?;
            let bearer = token(p).await?;
            let body = json!({"exclude_provider": optional(exclude)});
            http::post(&["email", "send", hash, "new-invoice"], &body, &bearer).await
        }
        "lnemail.renewalInvoice" => {
            allowed(p, &["accountId", "years"])?;
            let years = match p.get("years") {
                None | Some(Value::Null) => None,
                Some(Value::Number(n)) => {
                    Some(n.as_u64().filter(|y| (1..=10).contains(y)).ok_or("invalid_params")?)
                }
                _ => return Err("invalid_params"),
            };
            let bearer = token(p).await?;
            let body = match years {
                Some(years) => json!({"years": years}),
                None => json!({}),
            };
            http::post(&["account", "renew"], &body, &bearer).await
        }
        "lnemail.renewalStatus" => {
            allowed(p, &["accountId", "paymentHash"])?;
            let hash = segment(p, "paymentHash")?;
            let bearer = token(p).await?;
            http::get(&["account", "renew", "status", hash], &bearer).await
        }
        "lnemail.renewalReissue" => {
            allowed(p, &["accountId", "paymentHash", "excludeProvider"])?;
            let hash = segment(p, "paymentHash")?;
            let exclude = text(p, "excludeProvider", false, 256)?;
            let bearer = token(p).await?;
            let body = json!({"exclude_provider": optional(exclude)});
            http::post(&["account", "renew", hash, "new-invoice"], &body, &bearer).await
        }
        _ => Err("unknown_method"),
    }
}

#[cfg(test)]
#[path = "lnemail_tests.rs"]
mod tests;
