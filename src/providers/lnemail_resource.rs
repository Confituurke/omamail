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

fn capitalized(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A row in `GET /email/sends/recent`: LNemail's only record of outgoing
/// mail, and never a copy of what was actually sent — this is a status log,
/// not an archive.
pub fn sent_row(entry: &Value) -> Value {
    let hash = text(&entry["payment_hash"]);
    let id = format!("send-{hash}");
    let date = {
        let sent = text(&entry["sent_at"]);
        if sent.is_empty() { text(&entry["created_at"]) } else { sent }
    };
    let mut headers = Vec::new();
    header(&mut headers, "To", &text(&entry["recipient"]));
    header(&mut headers, "Subject", &text(&entry["subject"]));
    header(&mut headers, "Date", &date);
    json!({
        "id": id,
        "threadId": id,
        "labelIds": ["SENT"],
        "internalDate": internal_date(&date),
        "sizeEstimate": 0,
        "snippet": format!(
            "Payment {}, delivery {}",
            text(&entry["payment_status"]),
            text(&entry["delivery_status"])
        ),
        "payload": {
            "mimeType": "text/plain",
            "headers": headers,
            "body": {"size": 0},
            "parts": []
        }
    })
}

/// A send row opened as a message: there is no body to show, so the reader
/// is told why rather than shown nothing or a request that quietly fails.
pub fn sent_message(entry: &Value) -> Value {
    let mut row = sent_row(entry);
    let body = format!(
        "LNemail does not keep a copy of what you sent — only this record that it \
         was sent and whether it was delivered.\n\nPayment: {}\nDelivery: {}",
        capitalized(&text(&entry["payment_status"])),
        capitalized(&text(&entry["delivery_status"])),
    );
    row["payload"]["body"] = json!({"size": body.len(), "data": URL_SAFE_NO_PAD.encode(&body)});
    row["snippet"] = json!("");
    row
}

/// A send row opened as a message, its body recovered from this device's own
/// encrypted local cache rather than shown as the dummy `sent_message` above
/// gives — because this device is the one that sent it and chose to remember
/// it. Everything else about the row, including its date and ordering, still
/// comes from `entry`, LNemail's own record: the cache only ever supplies a
/// body, never anything a listing sorts or matches by.
pub fn sent_message_from_cache(entry: &Value, cached: &Value) -> Value {
    let mut row = sent_row(entry);
    let body = format!(
        "Kept on this device only — LNemail itself still keeps no copy.\n\n{}",
        text(&cached["body"])
    );
    row["payload"]["body"] = json!({"size": body.len(), "data": URL_SAFE_NO_PAD.encode(&body)});
    row["snippet"] = json!("");
    row
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

// --------------------------------------------------------------- outgoing
//
// `MailAccount` builds the same payload for every provider: a base64url `raw`
// field, a complete RFC 822 message with its attachments already embedded —
// that is what Gmail's own send endpoint takes, and what goes out over SMTP
// unchanged for IMAP and Outlook. LNemail's `POST /email/send` takes none of
// that; it wants a recipient, a subject, a plain-text body and attachments
// apart, so the message is taken apart again here and handed over as the
// fields the endpoint has — the same reason HEY's own send decodes `raw`
// before it ever reaches the CLI.

const MAX_OUTGOING_ATTACHMENT_TOTAL: usize = 8 * 1024 * 1024;
const MAX_OUTGOING_ATTACHMENTS: usize = 20;

fn header_value(mail: &mailparse::ParsedMail, name: &str) -> String {
    use mailparse::MailHeaderMap;
    mail.headers.get_first_value(name).unwrap_or_default()
}

fn addresses(mail: &mailparse::ParsedMail, name: &str) -> Vec<String> {
    let value = header_value(mail, name);
    if value.is_empty() {
        return Vec::new();
    }
    let Ok(list) = mailparse::addrparse(&value) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for addr in list.iter() {
        match addr {
            mailparse::MailAddr::Single(info) => out.push(info.addr.clone()),
            mailparse::MailAddr::Group(group) => {
                out.extend(group.addrs.iter().map(|info| info.addr.clone()));
            }
        }
    }
    out
}

/// LNemail's send takes exactly one recipient. Every address named across
/// To, Cc and Bcc has to be that one recipient or refused outright — sending
/// to the first and silently dropping the rest is mail that never reaches
/// someone the sender named.
fn single_recipient(mail: &mailparse::ParsedMail) -> Result<String, &'static str> {
    let mut all = addresses(mail, "To");
    all.extend(addresses(mail, "Cc"));
    all.extend(addresses(mail, "Bcc"));
    match all.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err("lnemail_recipient_required"),
        _ => Err("lnemail_single_recipient_only"),
    }
}

fn is_attachment(part: &mailparse::ParsedMail) -> bool {
    part.get_content_disposition().disposition == mailparse::DispositionType::Attachment
        || (!part.ctype.mimetype.starts_with("text/")
            && !part.ctype.mimetype.starts_with("multipart/"))
}

fn walk_outgoing(
    part: &mailparse::ParsedMail,
    depth: usize,
    plain: &mut Option<String>,
    attachments: &mut Vec<Value>,
) -> Result<(), &'static str> {
    if depth > 12 {
        return Ok(());
    }
    if !part.subparts.is_empty() {
        for child in &part.subparts {
            walk_outgoing(child, depth + 1, plain, attachments)?;
        }
        return Ok(());
    }
    if is_attachment(part) {
        if attachments.len() >= MAX_OUTGOING_ATTACHMENTS {
            return Err("invalid_params");
        }
        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .or_else(|| part.ctype.params.get("name"))
            .cloned()
            .unwrap_or_else(|| "attachment".to_owned());
        let bytes = part.get_body_raw().map_err(|_| "invalid_params")?;
        attachments.push(json!({
            "filename": filename,
            "content_type": part.ctype.mimetype,
            "content": STANDARD.encode(&bytes),
        }));
    } else if plain.is_none() && part.ctype.mimetype == "text/plain" {
        *plain = Some(part.get_body().map_err(|_| "invalid_params")?);
    }
    Ok(())
}

fn optional_header(value: String) -> Value {
    if value.is_empty() { Value::Null } else { Value::String(value) }
}

/// A complete RFC 822 message, taken apart into LNemail's own send fields.
pub fn decode_outgoing(bytes: &[u8]) -> Result<Value, &'static str> {
    let mail = mailparse::parse_mail(bytes).map_err(|_| "invalid_params")?;
    let recipient = single_recipient(&mail)?;
    let subject = header_value(&mail, "Subject");
    let in_reply_to = header_value(&mail, "In-Reply-To");
    let references = header_value(&mail, "References");
    let mut plain = None;
    let mut attachments = Vec::new();
    walk_outgoing(&mail, 0, &mut plain, &mut attachments)?;
    let total: usize = attachments
        .iter()
        .filter_map(|item| item["content"].as_str())
        .map(str::len)
        .sum();
    if total > MAX_OUTGOING_ATTACHMENT_TOTAL * 4 / 3 + attachments.len() * 4 {
        return Err("invalid_params");
    }
    Ok(json!({
        "recipient": recipient,
        "subject": subject,
        "body": plain.unwrap_or_default(),
        "in_reply_to": optional_header(in_reply_to),
        "references": optional_header(references),
        "attachments": attachments,
    }))
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

    #[test]
    fn a_plain_message_decodes_to_lnemail_s_own_send_fields() {
        let raw = b"To: a@example.org\r\nSubject: Hi\r\nIn-Reply-To: <m1@example.org>\r\n\
            References: <m0@example.org> <m1@example.org>\r\n\
            Content-Type: text/plain; charset=utf-8\r\n\r\nhello there";
        let fields = decode_outgoing(raw).unwrap();
        assert_eq!(fields["recipient"], "a@example.org");
        assert_eq!(fields["subject"], "Hi");
        assert_eq!(fields["body"], "hello there");
        assert_eq!(fields["in_reply_to"], "<m1@example.org>");
        assert_eq!(fields["references"], "<m0@example.org> <m1@example.org>");
        assert_eq!(fields["attachments"], json!([]));
    }

    #[test]
    fn no_recipient_or_more_than_one_across_to_cc_and_bcc_is_refused() {
        let none = b"Subject: Hi\r\n\r\nbody";
        assert_eq!(decode_outgoing(none), Err("lnemail_recipient_required"));
        let two = b"To: a@example.org\r\nCc: b@example.org\r\n\r\nbody";
        assert_eq!(decode_outgoing(two), Err("lnemail_single_recipient_only"));
    }

    #[test]
    fn a_multipart_message_keeps_the_plain_part_and_collects_attachments_by_disposition() {
        let raw = b"To: a@example.org\r\nSubject: Hi\r\n\
            Content-Type: multipart/mixed; boundary=x\r\n\r\n\
            --x\r\nContent-Type: multipart/alternative; boundary=y\r\n\r\n\
            --y\r\nContent-Type: text/plain\r\n\r\nplain body\r\n\
            --y\r\nContent-Type: text/html\r\n\r\n<p>html body</p>\r\n--y--\r\n\
            --x\r\nContent-Type: application/octet-stream\r\n\
            Content-Disposition: attachment; filename=blob.bin\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nAAEC\r\n--x--\r\n";
        let fields = decode_outgoing(raw).unwrap();
        assert_eq!(fields["body"], "plain body", "the HTML alternative is dropped, LNemail sends plain text only");
        let attachments = fields["attachments"].as_array().unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0]["filename"], "blob.bin");
        assert_eq!(attachments[0]["content_type"], "application/octet-stream");
        assert_eq!(
            STANDARD.decode(attachments[0]["content"].as_str().unwrap()).unwrap(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn a_missing_in_reply_to_and_references_are_null_not_empty_strings() {
        let raw = b"To: a@example.org\r\nSubject: Hi\r\n\r\nbody";
        let fields = decode_outgoing(raw).unwrap();
        assert_eq!(fields["in_reply_to"], Value::Null);
        assert_eq!(fields["references"], Value::Null);
    }

    #[test]
    fn attachments_over_the_combined_size_limit_are_refused() {
        // Base64 in the message itself, so it decodes to more than the cap —
        // encoded length runs a third larger than the decoded bytes it holds.
        let big = "A".repeat((MAX_OUTGOING_ATTACHMENT_TOTAL + 1024) * 4 / 3 + 4);
        let raw = format!(
            "To: a@example.org\r\nSubject: Hi\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n\
            --x\r\nContent-Type: text/plain\r\n\r\nbody\r\n\
            --x\r\nContent-Type: application/octet-stream\r\n\
            Content-Disposition: attachment; filename=big.bin\r\n\
            Content-Transfer-Encoding: base64\r\n\r\n{big}\r\n--x--\r\n"
        );
        assert_eq!(decode_outgoing(raw.as_bytes()), Err("invalid_params"));
    }

    #[test]
    fn malformed_bytes_are_refused_rather_than_panicking() {
        assert!(decode_outgoing(b"\xff\xfe not a message").is_err());
    }

    #[test]
    fn a_sent_row_names_itself_by_payment_hash_and_carries_no_body() {
        let row = sent_row(&json!({
            "payment_hash": "abc123",
            "recipient": "a@example.org",
            "subject": "Hi",
            "payment_status": "paid",
            "delivery_status": "sent",
            "created_at": "2026-09-13T12:00:00Z",
            "sent_at": "2026-09-13T12:00:05Z"
        }));
        assert_eq!(row["id"], "send-abc123");
        assert_eq!(row["threadId"], "send-abc123");
        assert_eq!(row["labelIds"], json!(["SENT"]));
        assert_ne!(row["internalDate"], "");
        assert_eq!(row["snippet"], "Payment paid, delivery sent");
        assert_eq!(row["payload"]["body"]["size"], 0);
        assert!(row["payload"]["body"].get("data").is_none());
        let headers = row["payload"]["headers"].as_array().unwrap();
        assert!(headers.contains(&json!({"name": "To", "value": "a@example.org"})));
        assert!(headers.contains(&json!({"name": "Subject", "value": "Hi"})));
    }

    #[test]
    fn a_sent_row_falls_back_to_created_at_when_never_confirmed_sent() {
        let row = sent_row(&json!({
            "payment_hash": "abc123", "recipient": "a@example.org", "subject": "Hi",
            "payment_status": "pending", "delivery_status": "pending",
            "created_at": "2026-09-13T12:00:00Z"
        }));
        assert_ne!(row["internalDate"], "");
    }

    #[test]
    fn opening_a_sent_row_explains_there_is_no_stored_body() {
        let message = sent_message(&json!({
            "payment_hash": "abc123", "recipient": "a@example.org", "subject": "Hi",
            "payment_status": "paid", "delivery_status": "failed",
            "created_at": "2026-09-13T12:00:00Z"
        }));
        assert_eq!(message["id"], "send-abc123");
        let body = URL_SAFE_NO_PAD
            .decode(message["payload"]["body"]["data"].as_str().unwrap())
            .unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.contains("does not keep a copy"));
        assert!(text.contains("Payment: Paid"));
        assert!(text.contains("Delivery: Failed"));
    }
}
