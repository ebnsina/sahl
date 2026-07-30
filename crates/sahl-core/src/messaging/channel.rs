//! Who is told, and by what route.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How a tenant reaches people.
///
/// A closed set: a channel is a provider integration with credentials and an approval process
/// behind it, not a string somebody can invent in a settings field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Channel {
    /// Dominant in the Gulf. Needs business verification and pre-approved templates, both of which
    /// have lead time — start the paperwork long before the feature is wanted.
    WhatsApp,
    /// Materially wider reach than WhatsApp in Bangladesh.
    Messenger,
    /// The floor everybody has. No templates, no verification, and it costs per message.
    Sms,
    /// For a digest nobody needs on their phone.
    Email,
}

impl Channel {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WhatsApp => "whatsapp",
            Self::Messenger => "messenger",
            Self::Sms => "sms",
            Self::Email => "email",
        }
    }

    /// Whether the provider requires the wording to be approved in advance.
    ///
    /// Drives whether a message can be composed freely or has to name a registered template. A
    /// send that ignores this is rejected by the provider, not by us — which is a failure nobody
    /// sees until a merchant asks why their receipts stopped.
    #[must_use]
    pub const fn needs_preapproved_template(self) -> bool {
        matches!(self, Self::WhatsApp)
    }

    /// Roughly how much room there is before a message is split or truncated.
    ///
    /// SMS is the one that matters: a segment is 160 characters and every one after the first is
    /// billed again, so a digest that ignores this is a bill nobody predicted.
    #[must_use]
    pub const fn soft_limit(self) -> usize {
        match self {
            Self::Sms => 160,
            Self::WhatsApp | Self::Messenger => 1_024,
            Self::Email => 100_000,
        }
    }

    /// Parse a stored label.
    ///
    /// # Errors
    /// The unrecognised label. Never falls back to a default — a tenant silently switched to SMS
    /// would be a bill, and one silently switched to WhatsApp would reach nobody in Dhaka.
    pub fn from_label(label: &str) -> Result<Self, String> {
        match label {
            "whatsapp" => Ok(Self::WhatsApp),
            "messenger" => Ok(Self::Messenger),
            "sms" => Ok(Self::Sms),
            "email" => Ok(Self::Email),
            other => Err(other.to_owned()),
        }
    }
}

/// Who a message is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recipient {
    /// Provider-specific: a phone number, a page-scoped id, an address. Opaque here on purpose —
    /// validating a phone number correctly is a per-country problem and belongs with the provider.
    pub address: String,
    pub channel: Channel,
}

/// Who a message concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// The person who owns the shop.
    Owner,
    /// Somebody who bought something.
    Customer,
}

/// What kind of message this is.
///
/// Carried separately from the text because a provider needs it — WhatsApp routes by template
/// name — and because an owner who has silenced low-stock alerts has not silenced their receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MessageKind {
    /// The day, once it is over.
    ClosingSummary,
    /// Something is running out.
    LowStock,
    /// A receipt, sent rather than printed.
    DigitalReceipt,
    /// Something in the anomaly feed worth interrupting somebody about.
    Anomaly,
}

impl MessageKind {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ClosingSummary => "closing_summary",
            Self::LowStock => "low_stock",
            Self::DigitalReceipt => "digital_receipt",
            Self::Anomaly => "anomaly",
        }
    }
}

/// A composed message, ready for whatever holds the socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub outlet_id: Uuid,
    pub kind: MessageKind,
    pub audience: Audience,
    /// Plain text. No markup: three providers render three different dialects of it, and a digest
    /// that arrives with asterisks in it looks broken rather than emphasised.
    pub body: String,
}

impl Message {
    /// Whether this would be split or truncated on `channel`.
    ///
    /// Checked rather than trimmed. Silently cutting a digest would remove the last figure, which
    /// is where the totals are.
    #[must_use]
    pub fn fits(&self, channel: Channel) -> bool {
        self.body.chars().count() <= channel.soft_limit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip_through_their_stored_label() {
        for channel in [
            Channel::WhatsApp,
            Channel::Messenger,
            Channel::Sms,
            Channel::Email,
        ] {
            assert_eq!(Channel::from_label(channel.label()), Ok(channel));
        }
    }

    #[test]
    fn an_unknown_channel_is_refused_rather_than_defaulted() {
        // Silently switched to SMS is a bill; silently switched to WhatsApp reaches nobody in
        // Dhaka. Neither is a thing to guess at.
        assert_eq!(Channel::from_label("telegram"), Err("telegram".to_owned()));
    }

    #[test]
    fn only_whatsapp_needs_its_wording_approved_in_advance() {
        assert!(Channel::WhatsApp.needs_preapproved_template());
        assert!(!Channel::Sms.needs_preapproved_template());
        assert!(!Channel::Messenger.needs_preapproved_template());
    }

    #[test]
    fn sms_is_the_channel_with_a_real_limit() {
        // A segment is 160 characters and every one after the first is billed again.
        assert_eq!(Channel::Sms.soft_limit(), 160);
        assert!(Channel::WhatsApp.soft_limit() > Channel::Sms.soft_limit());
    }

    #[test]
    fn a_message_reports_that_it_does_not_fit_rather_than_being_cut() {
        // Truncating a digest removes the last figure, which is where the totals are.
        let message = Message {
            outlet_id: Uuid::from_u128(1),
            kind: MessageKind::ClosingSummary,
            audience: Audience::Owner,
            body: "x".repeat(200),
        };

        assert!(!message.fits(Channel::Sms));
        assert!(message.fits(Channel::WhatsApp));
    }
}
