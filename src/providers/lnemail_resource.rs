//! Composes LNemail's own JSON into the shared Gmail message resource shape.
//!
//! LNemail already hands over parsed fields — a sender, a subject, plain and
//! HTML bodies apart, attachments with their bytes inline — never a MIME
//! stream to parse. So this builds the resource the same way HEY's own full
//! read does: field for field, not by round-tripping through a parser that
//! has nothing to parse.
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde_json::{Value, json};

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").trim().to_owned()
}

fn header(headers: &mut Vec<Value>, name: &str, value: &str) {
    if !value.is_empty() {
        headers.push(json!({"name": name, "value": value}));
    }
}

fn angle_bracketed(id: &str) -> String {
    if id.is_empty() || id.starts_with('<') {
        id.to_owned()
    } else {
        format!("<{id}>")
    }
}

/// The envelope headers every resource carries. `full` adds the two a list
/// row's `EmailHeader` never sends: LNemail names them only on a full read.
fn envelope_headers(entry: &Value, full: bool) -> Vec<Value> {
    let mut headers = Vec::new();
    header(&mut headers, "From", &text(&entry["sender"]));
    header(&mut headers, "To", &text(&entry["recipient"]));
    header(&mut headers, "Subject", &text(&entry["subject"]));
    header(&mut headers, "Date", &text(&entry["date"]));
    if full {
        let message_id = text(&entry["message_id"]);
        if !message_id.is_empty() {
            header(&mut headers, "Message-ID", &angle_bracketed(&message_id));
        }
        header(&mut headers, "References", &text(&entry["references"]));
    }
    headers
}

fn label_ids(read: bool) -> Vec<&'static str> {
    let mut ids = vec!["INBOX"];
    if !read {
        ids.push("UNREAD");
    }
    ids
}

/// LNemail's `date` may be an RFC 3339 timestamp or a raw mail `Date` header;
/// this is the same two-step fallback HEY's own resource uses for the same
/// reason — one API, both shapes seen in practice.
fn internal_date(date: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(date)
        .map(|d| d.timestamp_millis())
        .ok()
        .or_else(|| {
            mailparse::dateparse(date)
                .ok()
                .and_then(|v| v.checked_mul(1000))
        })
        .filter(|v| *v > 0)
        .map(|v| v.to_string())
        .unwrap_or_default()
}

/// A list row from `GET /emails`: headers only, since `EmailHeader` carries
/// no body of its own for a snippet or a reader to draw from.
pub fn list_row(entry: &Value) -> Value {
    let id = text(&entry["id"]);
    let read = entry["read"] == true;
    json!({
        "id": id,
        "threadId": id,
        "labelIds": label_ids(read),
        "internalDate": internal_date(&text(&entry["date"])),
        "sizeEstimate": 0,
        "snippet": "",
        "payload": {
            "mimeType": "text/plain",
            "headers": envelope_headers(entry, false),
            "body": {"size": 0},
            "parts": []
        }
    })
}

fn attachment_bytes(item: &Value) -> Vec<u8> {
    let content = text(&item["content"]);
    if text(&item["encoding"]) == "base64" {
        STANDARD.decode(&content).unwrap_or_default()
    } else {
        content.into_bytes()
    }
}

fn attachment_part(index: usize, item: &Value) -> Value {
    let filename = text(&item["filename"]);
    let mime_type = text(&item["content_type"]);
    let quoted = filename.replace('\\', "\\\\").replace('"', "\\\"");
    json!({
        "partId": index.to_string(),
        "mimeType": if mime_type.is_empty() { "application/octet-stream".to_owned() } else { mime_type },
        "filename": filename,
        "headers": [{"name": "Content-Disposition", "value": format!("attachment; filename=\"{quoted}\"")}],
        "body": {"size": attachment_bytes(item).len(), "attachmentId": index.to_string()},
        "parts": []
    })
}

fn text_part(mime_type: &str, body: &str) -> Value {
    json!({
        "mimeType": mime_type,
        "headers": [{"name": "Content-Type", "value": format!("{mime_type}; charset=utf-8")}],
        "body": {"size": body.len(), "data": URL_SAFE_NO_PAD.encode(body)},
        "parts": []
    })
}

/// A full read from `GET /emails/{id}`: the multipart tree LNemail never
/// sends over its own wire, built from bodies it already gives apart.
pub fn full_message(entry: &Value) -> Value {
    let id = text(&entry["id"]);
    let read = entry["read"] == true;
    let plain = {
        let value = text(&entry["body_plain"]);
        if value.is_empty() { text(&entry["body"]) } else { value }
    };
    let html = text(&entry["body_html"]);

    let text_body = if html.is_empty() {
        text_part("text/plain", &plain)
    } else if plain.is_empty() {
        text_part("text/html", &html)
    } else {
        json!({
            "mimeType": "multipart/alternative",
            "headers": [],
            "body": {"size": 0},
            "parts": [text_part("text/plain", &plain), text_part("text/html", &html)]
        })
    };

    let attachments = entry["attachments"].as_array().cloned().unwrap_or_default();
    let mut payload = if attachments.is_empty() {
        text_body
    } else {
        let mut parts = vec![text_body];
        for (index, item) in attachments.iter().enumerate() {
            parts.push(attachment_part(index, item));
        }
        json!({
            "mimeType": "multipart/mixed",
            "headers": [],
            "body": {"size": 0},
            "parts": parts
        })
    };

    // The top level alone carries the envelope: a nested alternative or
    // mixed wrapper keeps its own Content-Type, but From/To/Subject/Date name
    // the message once, not once per part.
    let top_mime = text(&payload["mimeType"]);
    let mut headers = envelope_headers(entry, true);
    headers.push(json!({"name": "Content-Type", "value": format!("{top_mime}; charset=utf-8")}));
    payload["headers"] = json!(headers);

    let size = attachments.iter().map(|item| attachment_bytes(item).len()).sum::<usize>()
        + plain.len()
        + html.len();

    json!({
        "id": id,
        "threadId": id,
        "labelIds": label_ids(read),
        "internalDate": internal_date(&text(&entry["date"])),
        "sizeEstimate": size,
        "snippet": "",
        "payload": payload
    })
}

/// One attachment's bytes, base64url, for `lnemail.attachment`. LNemail hands
/// every attachment back inline on the full read and has no endpoint to fetch
/// one from separately, so this re-reads the message and pulls it out by the
/// index `full_message` named it with — a fresh, small, targeted read rather
/// than a cache this provider would otherwise be the only one keeping.
pub fn attachment_data(entry: &Value, index: usize) -> Result<Value, &'static str> {
    let item = entry["attachments"].get(index).ok_or("lnemail_not_found")?;
    let bytes = attachment_bytes(item);
    Ok(json!({"size": bytes.len(), "data": URL_SAFE_NO_PAD.encode(&bytes)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_row_carries_headers_and_an_unread_label_but_no_body() {
        let row = list_row(&json!({
            "id": "abc", "subject": "Hi", "sender": "a@example.org",
            "recipient": "me@lnemail.net", "date": "2026-09-13T12:00:00Z", "read": false
        }));
        assert_eq!(row["id"], "abc");
        assert_eq!(row["threadId"], "abc");
        assert_eq!(row["labelIds"], json!(["INBOX", "UNREAD"]));
        assert_eq!(row["payload"]["mimeType"], "text/plain");
        assert_eq!(row["payload"]["body"]["size"], 0);
        assert!(row["payload"]["body"].get("data").is_none());
        let headers = row["payload"]["headers"].as_array().unwrap();
        assert!(headers.contains(&json!({"name": "From", "value": "a@example.org"})));
        assert!(headers.contains(&json!({"name": "Subject", "value": "Hi"})));
        assert!(!headers.iter().any(|h| h["name"] == "Message-ID"));
    }

    #[test]
    fn a_read_message_carries_no_unread_label() {
        let row = list_row(&json!({"id": "x", "read": true}));
        assert_eq!(row["labelIds"], json!(["INBOX"]));
    }

    #[test]
    fn a_plain_only_message_is_one_leaf_part_base64url_encoded() {
        let message = full_message(&json!({
            "id": "m1", "sender": "a@x.org", "subject": "Hi", "date": "2026-09-13T12:00:00Z",
            "body_plain": "hello\nworld", "read": true
        }));
        assert_eq!(message["payload"]["mimeType"], "text/plain");
        let data = message["payload"]["body"]["data"].as_str().unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(data).unwrap(), b"hello\nworld");
        assert!(message["payload"]["parts"].as_array().unwrap().is_empty());
        assert!(
            message["payload"]["headers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["name"] == "Content-Type" && h["value"] == "text/plain; charset=utf-8")
        );
    }

    #[test]
    fn plain_and_html_become_a_multipart_alternative() {
        let message = full_message(&json!({
            "id": "m1", "body_plain": "hi", "body_html": "<p>hi</p>", "read": true
        }));
        assert_eq!(message["payload"]["mimeType"], "multipart/alternative");
        let parts = message["payload"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["mimeType"], "text/plain");
        assert_eq!(parts[1]["mimeType"], "text/html");
        assert_eq!(
            URL_SAFE_NO_PAD.decode(parts[1]["body"]["data"].as_str().unwrap()).unwrap(),
            b"<p>hi</p>"
        );
    }

    #[test]
    fn falls_back_to_body_when_body_plain_is_absent() {
        let message = full_message(&json!({"id": "m1", "body": "fallback", "read": true}));
        assert_eq!(message["payload"]["mimeType"], "text/plain");
        assert_eq!(
            URL_SAFE_NO_PAD.decode(message["payload"]["body"]["data"].as_str().unwrap()).unwrap(),
            b"fallback"
        );
    }

    #[test]
    fn attachments_wrap_everything_in_multipart_mixed_and_name_themselves_by_index() {
        let message = full_message(&json!({
            "id": "m1", "body_plain": "hi", "read": false,
            "attachments": [
                {"filename":"a.txt","content_type":"text/plain","content":"aGVsbG8=","encoding":"base64"},
                {"filename":"b.txt","content_type":"text/plain","content":"plain text","encoding":"text"}
            ]
        }));
        assert_eq!(message["payload"]["mimeType"], "multipart/mixed");
        let parts = message["payload"]["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["mimeType"], "text/plain");
        assert_eq!(parts[1]["filename"], "a.txt");
        assert_eq!(parts[1]["body"]["attachmentId"], "0");
        assert_eq!(parts[1]["body"]["size"], 5, "aGVsbG8= decodes to 5 bytes");
        assert_eq!(parts[2]["body"]["attachmentId"], "1");
        assert_eq!(parts[2]["body"]["size"], "plain text".len());
        assert_eq!(
            parts[1]["headers"][0]["value"],
            "attachment; filename=\"a.txt\""
        );
    }

    #[test]
    fn a_quote_or_backslash_in_a_filename_cannot_end_the_disposition_value_early() {
        let part = attachment_part(0, &json!({"filename": "a\"b\\c", "content": "", "encoding": "text"}));
        assert_eq!(
            part["headers"][0]["value"],
            "attachment; filename=\"a\\\"b\\\\c\""
        );
    }

    #[test]
    fn attachment_data_decodes_by_index_and_a_bad_index_is_refused() {
        let entry = json!({"attachments": [
            {"filename":"a.bin","content":"AAEC","encoding":"base64"}
        ]});
        let value = attachment_data(&entry, 0).unwrap();
        assert_eq!(value["size"], 3);
        assert_eq!(
            URL_SAFE_NO_PAD.decode(value["data"].as_str().unwrap()).unwrap(),
            vec![0, 1, 2]
        );
        assert_eq!(attachment_data(&entry, 1), Err("lnemail_not_found"));
    }

    #[test]
    fn an_iso_date_and_a_raw_mail_date_header_both_resolve() {
        assert_ne!(internal_date("2026-09-13T12:00:00Z"), "");
        assert_ne!(internal_date("Sun, 13 Sep 2026 12:00:00 +0000"), "");
        assert_eq!(internal_date(""), "");
        assert_eq!(internal_date("not a date"), "");
    }
}
