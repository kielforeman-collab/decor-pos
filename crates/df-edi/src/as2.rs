//! AS2 message packaging and MDN handling (Phase 4, v0.5).
//!
//! ## What is real and what is not
//!
//! Real in this module:
//!
//! * [`As2Message`] — the AS2 envelope as data: `AS2-From` /
//!   `AS2-To`, `Message-ID`, MIME version, content type, and the
//!   EDI payload, rendered as MIME headers + payload and parsed
//!   back.
//! * [`Mdn`] — a Message Disposition Notification as data:
//!   the original message id, the disposition
//!   (`processed` / `failed`), and the reporting user agent,
//!   rendered as a minimal `multipart/report` and parsed back.
//!
//! Honestly **not** implemented, and never faked:
//!
//! * S/MIME signing and encryption. [`sign_payload`] and
//!   [`encrypt_payload`] return [`EdiError::NotSupported`]. Real
//!   AS2 signing/encryption needs certificates and an S/MIME
//!   implementation, which is out of scope for this VM build; a
//!   message built here is **unsigned and unencrypted** and says so
//!   in its `As2Message.security` field.
//! * Network delivery. There is no HTTP POST to a vendor here —
//!   an `As2Message` is handed to a [`crate::transport::VendorTransport`]
//!   (e.g. the local drop folder) as a file, or to a vendor
//!   plugin that owns the HTTP exchange. No live vendor endpoint
//!   is ever contacted by this crate.

use crate::error::{EdiError, Result};

/// What (if any) S/MIME protection an AS2 message carries.
///
/// Only `None` is implemented. Signing and encryption are
/// `NotSupported` — see [`sign_payload`] / [`encrypt_payload`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As2Security {
    /// Unsigned, unencrypted payload (the only form this crate
    /// can produce — suitable for a test drop folder, never
    /// presented as a production AS2 exchange).
    None,
    /// S/MIME signed — not implemented.
    Signed,
    /// S/MIME encrypted — not implemented.
    Encrypted,
}

impl As2Security {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Signed => "signed",
            Self::Encrypted => "encrypted",
        }
    }
}

/// An AS2 message: MIME headers plus the EDI payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct As2Message {
    pub message_id: String,
    pub from: String,
    pub to: String,
    /// MIME content type of the payload, e.g. `application/edi-x12`.
    pub content_type: String,
    /// Who to send the MDN to, when the receiver should acknowledge.
    pub disposition_notification_to: Option<String>,
    /// Whether an MDN was requested (`Disposition-Notification-Options`).
    pub request_mdn: bool,
    pub security: As2Security,
    /// The raw EDI payload (e.g. an X12 850 document).
    pub payload: String,
}

/// Build an AS2 message for a payload. The `message_id` must be
/// globally unique (callers typically use a UUID); it is carried
/// verbatim into the MDN so the sender can correlate the receipt.
pub fn build_as2_message(message_id: &str, from: &str, to: &str, payload: &str) -> As2Message {
    As2Message {
        message_id: message_id.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        content_type: "application/edi-x12".to_string(),
        disposition_notification_to: Some(from.to_string()),
        request_mdn: true,
        security: As2Security::None,
        payload: payload.to_string(),
    }
}

/// Render an AS2 message as MIME headers, a blank line, and the
/// payload. The `security: none` fact is carried in an
/// `X-DF-EDI-Security` header so a reader never mistakes the file
/// for a signed/encrypted exchange.
pub fn render_as2_message(message: &As2Message) -> String {
    let mut out = String::new();
    out.push_str("MIME-Version: 1.0\n");
    out.push_str(&format!("Message-ID: {}\n", message.message_id));
    out.push_str(&format!("AS2-From: {}\n", message.from));
    out.push_str(&format!("AS2-To: {}\n", message.to));
    out.push_str(&format!("Content-Type: {}\n", message.content_type));
    out.push_str(&format!(
        "X-DF-EDI-Security: {}\n",
        message.security.as_str()
    ));
    if let Some(notify_to) = &message.disposition_notification_to {
        out.push_str(&format!("Disposition-Notification-To: {notify_to}\n"));
    }
    if message.request_mdn {
        out.push_str(
            "Disposition-Notification-Options: signed-receipt-protocol=optional-pkcs7-signature; signed-receipt-micalg=optional-sha256\n",
        );
    }
    out.push('\n');
    out.push_str(&message.payload);
    out
}

/// Parse a rendered AS2 message back into its parts. Headers are
/// matched case-insensitively; the payload is everything after the
/// first blank line. A missing `Message-ID`, `AS2-From` or `AS2-To`
/// is an error.
pub fn parse_as2_message(text: &str) -> Result<As2Message> {
    let (header_part, payload) = match text.split_once("\n\n") {
        Some((headers, payload)) => (headers, payload),
        None => {
            return Err(EdiError::Parse(
                "AS2 message has no blank line separating headers from payload".to_string(),
            ));
        }
    };
    let mut message_id = None;
    let mut from = None;
    let mut to = None;
    let mut content_type = None;
    let mut disposition_notification_to = None;
    let mut request_mdn = false;
    for line in header_part.lines() {
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name.trim().to_ascii_lowercase(), value.trim().to_string()),
            None => continue,
        };
        match name.as_str() {
            "message-id" => message_id = Some(value),
            "as2-from" => from = Some(value),
            "as2-to" => to = Some(value),
            "content-type" => content_type = Some(value),
            "disposition-notification-to" => disposition_notification_to = Some(value),
            "disposition-notification-options" => request_mdn = true,
            _ => {}
        }
    }
    Ok(As2Message {
        message_id: message_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdiError::Parse("AS2 message is missing Message-ID".to_string()))?,
        from: from
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdiError::Parse("AS2 message is missing AS2-From".to_string()))?,
        to: to
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdiError::Parse("AS2 message is missing AS2-To".to_string()))?,
        content_type: content_type.unwrap_or_else(|| "application/edi-x12".to_string()),
        disposition_notification_to,
        request_mdn,
        security: As2Security::None,
        payload: payload.to_string(),
    })
}

/// Whether the receiver processed the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MdnDisposition {
    /// `automatic-action/MDN-sent-automatically; processed`
    Processed,
    /// `automatic-action/MDN-sent-automatically; failed` with a
    /// human-readable reason.
    Failed(String),
}

impl MdnDisposition {
    pub fn as_header_value(&self) -> String {
        match self {
            Self::Processed => "automatic-action/MDN-sent-automatically; processed".to_string(),
            Self::Failed(_) => "automatic-action/MDN-sent-automatically; failed".to_string(),
        }
    }
}

/// A Message Disposition Notification answering one AS2 message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mdn {
    pub message_id: String,
    pub original_message_id: String,
    pub disposition: MdnDisposition,
    pub reporting_ua: String,
}

/// Build an MDN answering `original_message_id`.
pub fn build_mdn(original_message_id: &str, disposition: MdnDisposition) -> Mdn {
    Mdn {
        message_id: format!("<mdn-{}@decor-pos>", uuid::Uuid::now_v7()),
        original_message_id: original_message_id.to_string(),
        disposition,
        reporting_ua: "decor-pos df-edi 0.5.0".to_string(),
    }
}

/// Render an MDN as a minimal `multipart/report`: headers, a
/// human-readable text part, and the `message/disposition` part
/// carrying `Original-Message-ID` and `Disposition`.
pub fn render_mdn(mdn: &Mdn) -> String {
    let mut out = String::new();
    out.push_str("MIME-Version: 1.0\n");
    out.push_str(&format!("Message-ID: {}\n", mdn.message_id));
    out.push_str("Content-Type: multipart/report; report-type=disposition-notification; boundary=\"DF-MDN\"\n");
    out.push('\n');
    out.push_str("--DF-MDN\n");
    out.push_str("Content-Type: text/plain\n");
    out.push('\n');
    match &mdn.disposition {
        MdnDisposition::Processed => {
            out.push_str("The message was processed successfully.\n");
        }
        MdnDisposition::Failed(reason) => {
            out.push_str(&format!("The message failed to process: {reason}\n"));
        }
    }
    out.push_str("--DF-MDN\n");
    out.push_str("Content-Type: message/disposition\n");
    out.push('\n');
    out.push_str(&format!("Reporting-UA: {}\n", mdn.reporting_ua));
    out.push_str(&format!(
        "Original-Message-ID: {}\n",
        mdn.original_message_id
    ));
    out.push_str(&format!(
        "Disposition: {}\n",
        mdn.disposition.as_header_value()
    ));
    out.push_str("--DF-MDN--\n");
    out
}

/// Parse a rendered MDN back into its parts. Requires an
/// `Original-Message-ID` and a `Disposition` header in the
/// disposition part; the disposition must be `processed` or
/// `failed`.
pub fn parse_mdn(text: &str) -> Result<Mdn> {
    let mut message_id = None;
    let mut original_message_id = None;
    let mut disposition: Option<MdnDisposition> = None;
    let mut reporting_ua = String::new();
    for line in text.lines() {
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name.trim().to_ascii_lowercase(), value.trim().to_string()),
            None => continue,
        };
        match name.as_str() {
            "message-id" => message_id = Some(value),
            "original-message-id" => original_message_id = Some(value),
            "reporting-ua" => reporting_ua = value,
            "disposition" => {
                if value.contains("; processed") {
                    disposition = Some(MdnDisposition::Processed);
                } else if value.contains("; failed") {
                    disposition = Some(MdnDisposition::Failed(
                        "vendor reported failure (no reason given in the MDN)".to_string(),
                    ));
                } else {
                    return Err(EdiError::Parse(format!(
                        "unrecognized MDN disposition {value:?}"
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(Mdn {
        message_id: message_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdiError::Parse("MDN is missing Message-ID".to_string()))?,
        original_message_id: original_message_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| EdiError::Parse("MDN is missing Original-Message-ID".to_string()))?,
        disposition: disposition
            .ok_or_else(|| EdiError::Parse("MDN is missing a Disposition".to_string()))?,
        reporting_ua,
    })
}

/// S/MIME-sign an AS2 payload — **not implemented**. Returns
/// [`EdiError::NotSupported`]; callers must not treat the
/// unsigned message as signed.
pub fn sign_payload(_payload: &str) -> Result<String> {
    Err(EdiError::NotSupported(
        "AS2 S/MIME signing is not implemented in this build (no certificates, no S/MIME library); messages are unsigned — see as2 module docs".to_string(),
    ))
}

/// S/MIME-encrypt an AS2 payload — **not implemented**. Returns
/// [`EdiError::NotSupported`]; callers must not treat the
/// unencrypted message as encrypted.
pub fn encrypt_payload(_payload: &str) -> Result<String> {
    Err(EdiError::NotSupported(
        "AS2 S/MIME encryption is not implemented in this build (no certificates, no S/MIME library); messages are unencrypted — see as2 module docs".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as2_message_round_trip() {
        let message = build_as2_message(
            "<msg-1@decor-pos>",
            "DECORPOS",
            "VENDOR-A",
            "ST*850*1~BEG*00*NE*PO-1**20261125~",
        );
        assert_eq!(message.security, As2Security::None);
        assert!(message.request_mdn);
        let text = render_as2_message(&message);
        assert!(text.contains("AS2-From: DECORPOS"));
        assert!(text.contains("AS2-To: VENDOR-A"));
        assert!(text.contains("X-DF-EDI-Security: none"));
        assert!(text.contains("ST*850*1~"));
        let back = parse_as2_message(&text).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn mdn_round_trip_processed_and_failed() {
        let mdn = build_mdn("<msg-1@decor-pos>", MdnDisposition::Processed);
        let text = render_mdn(&mdn);
        assert!(text.contains("Original-Message-ID: <msg-1@decor-pos>"));
        assert!(text.contains("Disposition: automatic-action/MDN-sent-automatically; processed"));
        let back = parse_mdn(&text).unwrap();
        assert_eq!(back.original_message_id, "<msg-1@decor-pos>");
        assert_eq!(back.disposition, MdnDisposition::Processed);

        let failed = build_mdn(
            "<msg-2@decor-pos>",
            MdnDisposition::Failed("bad payload".to_string()),
        );
        let back = parse_mdn(&render_mdn(&failed)).unwrap();
        assert!(matches!(back.disposition, MdnDisposition::Failed(_)));
    }

    #[test]
    fn crypto_is_honestly_not_supported() {
        assert!(matches!(
            sign_payload("ST*850*1~"),
            Err(EdiError::NotSupported(_))
        ));
        assert!(matches!(
            encrypt_payload("ST*850*1~"),
            Err(EdiError::NotSupported(_))
        ));
    }

    #[test]
    fn parse_as2_rejects_missing_headers() {
        assert!(parse_as2_message("Message-ID: <m>\n\nST*850*1~").is_err());
        assert!(parse_as2_message("no blank line at all").is_err());
        assert!(parse_mdn("Message-ID: <mdn>\nDisposition: automatic-action/MDN-sent-automatically; processed\n").is_err());
    }
}
