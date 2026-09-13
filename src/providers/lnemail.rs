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
use serde_json::{Value, json};

#[path = "lnemail_resource.rs"]
mod resource;

const MAX_TEXT: usize = 1024 * 1024;
const MAX_SHORT: usize = 4096;
const MAX_ATTACHMENT_TOTAL: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 20;
const MAX_DELETE_BATCH: usize = 500;

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

fn email_address(value: &str) -> Result<(), &'static str> {
    let Some((local, domain)) = value.split_once('@') else {
        return Err("invalid_params");
    };
    if local.is_empty()
        || domain.is_empty()
        || value.len() > 320
        || !domain.contains('.')
        || value.bytes().any(|b| b.is_ascii_whitespace())
    {
        return Err("invalid_params");
    }
    Ok(())
}

fn attachments(p: &Value) -> Result<Vec<Value>, &'static str> {
    let list = match p.get("attachments") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(list)) => list,
        _ => return Err("invalid_params"),
    };
    if list.len() > MAX_ATTACHMENTS {
        return Err("invalid_params");
    }
    let mut total = 0usize;
    let mut out = Vec::with_capacity(list.len());
    for item in list {
        let filename = text(item, "filename", true, 512)?;
        let content_type = text(item, "contentType", true, 256)?;
        let content = text(item, "content", true, MAX_ATTACHMENT_TOTAL)?;
        total = total
            .checked_add(content.len())
            .filter(|total| *total <= MAX_ATTACHMENT_TOTAL)
            .ok_or("invalid_params")?;
        out.push(json!({
            "filename": filename,
            "content_type": content_type,
            "content": content,
        }));
    }
    Ok(out)
}

async fn token(p: &Value) -> Result<String, &'static str> {
    let account = p.get("accountId").and_then(Value::as_str).ok_or("invalid_params")?;
    crate::auth::password("lnemail", account).await
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
            allowed(p, &["accountId"])?;
            let bearer = token(p).await?;
            http::get(&["account"], &bearer).await
        }
        "lnemail.list" => {
            allowed(p, &["accountId"])?;
            let bearer = token(p).await?;
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
            let answer = http::get(&["emails", id], &bearer).await?;
            Ok(resource::full_message(&answer))
        }
        "lnemail.attachment" => {
            allowed(p, &["accountId", "id", "attachmentId"])?;
            let id = segment(p, "id")?;
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
                checked.push(segment(&json!({"id": ids[index]}), "id")?.to_owned());
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
            allowed(
                p,
                &[
                    "accountId",
                    "recipient",
                    "subject",
                    "body",
                    "inReplyTo",
                    "references",
                    "attachments",
                ],
            )?;
            let recipient = text(p, "recipient", true, MAX_SHORT)?;
            email_address(recipient)?;
            let subject = text(p, "subject", true, MAX_SHORT)?;
            let email_body = text(p, "body", true, MAX_TEXT)?;
            let in_reply_to = text(p, "inReplyTo", false, MAX_SHORT)?;
            let references = text(p, "references", false, MAX_TEXT)?;
            let files = attachments(p)?;
            let bearer = token(p).await?;
            let body = json!({
                "recipient": recipient,
                "subject": subject,
                "body": email_body,
                "in_reply_to": optional(in_reply_to),
                "references": optional(references),
                "attachments": files,
            });
            http::post(&["email", "send"], &body, &bearer).await
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
        _ => Err("unknown_method"),
    }
}

#[cfg(test)]
#[path = "lnemail_tests.rs"]
mod tests;
