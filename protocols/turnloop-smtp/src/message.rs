//! Deterministic lettre message building. Caller supplies Date, Message-ID and
//! MIME boundary seed; this wrapper never calls lettre's clock/random defaults.
use crate::{Error, error};
pub use lettre::{
    Message,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header},
};
use std::time::SystemTime;
pub struct FileAttachment {
    pub filename: String,
    pub content_type: String,
    pub content: Vec<u8>,
}
pub struct Mail {
    pub from: Mailbox,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<FileAttachment>,
    pub headers: Vec<(String, String)>,
    pub date: SystemTime,
    pub message_id: String,
    /// ASCII alphanumeric seed, 1..=48 bytes. Caller ensures it is absent from
    /// content; derived mixed/alternative boundaries are distinct.
    pub boundary_seed: String,
}
fn bad(message: &str) -> Error {
    error("EMESSAGE", "API", message, None, "")
}
fn multipart(kind: &str, seed: &str) -> Result<lettre::message::MultiPartBuilder, Error> {
    let content_type = header::ContentType::parse(&format!(
        "multipart/{kind}; boundary=\"turnloop-{kind}-{seed}\""
    ))
    .map_err(|e| bad(&e.to_string()))?;
    Ok(MultiPart::builder().header(content_type))
}
pub fn build(mut mail: Mail) -> Result<Message, Error> {
    if mail.boundary_seed.is_empty()
        || mail.boundary_seed.len() > 48
        || !mail
            .boundary_seed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric())
    {
        return Err(bad("Invalid MIME boundary seed"));
    }
    if mail.message_id.is_empty() || mail.message_id.contains(['\r', '\n']) {
        return Err(bad("Invalid Message-ID"));
    }
    let mut builder = Message::builder()
        .from(mail.from)
        .subject(mail.subject)
        .date(mail.date)
        .message_id(Some(mail.message_id));
    for to in mail.to {
        builder = builder.to(to);
    }
    for cc in mail.cc {
        builder = builder.cc(cc);
    }
    for bcc in mail.bcc {
        builder = builder.bcc(bcc);
    }
    for (name, value) in mail.headers {
        if value.contains(['\r', '\n'])
            || [
                "date",
                "message-id",
                "from",
                "to",
                "cc",
                "bcc",
                "mime-version",
                "content-type",
                "content-transfer-encoding",
            ]
            .iter()
            .any(|reserved| name.eq_ignore_ascii_case(reserved))
        {
            return Err(bad("Invalid or reserved custom header"));
        }
        let name = header::HeaderName::new_from_ascii(name).map_err(|e| bad(&e.to_string()))?;
        builder = builder.raw_header(header::HeaderValue::new(name, value));
    }
    let alternative = mail.text.is_some() && mail.html.is_some();
    let parts = if alternative {
        Some(
            multipart("alternative", &mail.boundary_seed)?
                .singlepart(SinglePart::plain(mail.text.take().unwrap()))
                .singlepart(SinglePart::html(mail.html.take().unwrap())),
        )
    } else {
        None
    };
    if mail.attachments.is_empty() {
        if let Some(parts) = parts {
            builder.multipart(parts)
        } else if let Some(html) = mail.html {
            builder.singlepart(SinglePart::html(html))
        } else {
            builder.singlepart(SinglePart::plain(mail.text.unwrap_or_default()))
        }
    } else {
        let mixed = multipart("mixed", &mail.boundary_seed)?;
        let mut mixed = if let Some(parts) = parts {
            mixed.multipart(parts)
        } else if let Some(html) = mail.html {
            mixed.singlepart(SinglePart::html(html))
        } else {
            mixed.singlepart(SinglePart::plain(mail.text.unwrap_or_default()))
        };
        for attachment in mail.attachments {
            let content_type = header::ContentType::parse(&attachment.content_type)
                .map_err(|e| bad(&e.to_string()))?;
            mixed = mixed.singlepart(
                Attachment::new(attachment.filename).body(attachment.content, content_type),
            );
        }
        builder.multipart(mixed)
    }
    .map_err(|e| bad(&e.to_string()))
}
