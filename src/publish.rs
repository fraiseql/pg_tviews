//! Publishing a committed transaction's changed TVIEW rows on `tviews_changes`
//! (ADR 0231).
//!
//! The transaction callback calls [`at_pre_commit`]. It encodes the
//! transaction's publish set ([`crate::queue::published`]) into messages of one
//! shape, version 1:
//!
//! ```json
//! {"v":1,"u":{"User":["8f…"]},"d":{"Comment":["1c…"]},"t":["Tag"]}
//! ```
//!
//! `u` lists the rows inserted or updated by type, `d` the rows deleted, `t` the
//! types whose rows are not listed. Nothing here may raise: an error at
//! `PRE_COMMIT` would roll back the user's transaction.

use crate::queue::published::{self, Folded, Publish};
use pgrx::prelude::*;
use std::cell::Cell;
use std::collections::BTreeSet;

/// The messages one transaction sends at most: past them, the remaining types go
/// out by name in the last one.
pub const MAX_MESSAGES: usize = 8;

/// The bytes one message carries at most: under PostgreSQL's
/// `NOTIFY_PAYLOAD_MAX_LENGTH` (`BLCKSZ - NAMEDATALEN - 128`: 7,999 with the
/// default 8 kB blocks), with room to spare.
pub const MESSAGE_BYTES: usize = pg_sys::BLCKSZ as usize - pg_sys::NAMEDATALEN as usize - 128 - 99;

/// The channel every message goes to.
pub const CHANNEL: &std::ffi::CStr = c"tviews_changes";

/// Above this share of the NOTIFY queue in use, a transaction commits unpublished:
/// a listener stuck in a transaction would otherwise end up failing every write.
const QUEUE_USAGE_LIMIT: f64 = 0.9;

thread_local! {
    /// The transactions this session committed unpublished (`publish_skipped`).
    static SKIPPED: Cell<u64> = const { Cell::new(0) };
}

/// The transactions this session committed without publishing their changes,
/// the NOTIFY queue being nearly full.
pub fn skipped() -> u64 {
    SKIPPED.get()
}

/// Publish the committing transaction's changes. Runs in the transaction callback
/// at `PRE_COMMIT`, after the refresh queue is known empty: no SPI, and nothing
/// that can raise but PostgreSQL's own NOTIFY in a race on a full queue.
pub fn at_pre_commit() {
    let Some(publish) = crate::config::publish_changes() else {
        return;
    };
    if published::is_empty() {
        return;
    }
    // SAFETY: a built-in function taking no argument, called in a transaction.
    let usage =
        unsafe { pgrx::direct_function_call::<f64>(pg_sys::pg_notification_queue_usage, &[]) }
            .unwrap_or(0.0);
    if usage > QUEUE_USAGE_LIMIT {
        SKIPPED.set(SKIPPED.get() + 1);
        warning!(
            "pg_tviews: the NOTIFY queue is {:.0}% full: this transaction's TVIEW changes are \
             not published on {}. A listener is not reading: see pg_notification_queue_usage() \
             and the sessions idle in a transaction in pg_stat_activity",
            usage * 100.0,
            CHANNEL.to_string_lossy()
        );
        return;
    }
    let channel = CHANNEL.to_string_lossy();
    for message in published::with_folded(|folded| encode(folded, publish)) {
        // SAFETY: pg_notify(text, text), with two non-null text datums, called in
        // a transaction that is not a parallel worker's (the callback ignores
        // those events). The message is under NOTIFY's payload limit.
        unsafe {
            pgrx::direct_function_call::<()>(
                pg_sys::pg_notify,
                &[channel.as_ref().into_datum(), message.as_str().into_datum()],
            );
        }
    }
}

/// Refuse to prepare a transaction with changes to publish (ADR 0231): PostgreSQL
/// cannot carry a notification through PREPARE, and committing it unpublished would
/// leave caches stale with no signal. Runs at `PRE_PREPARE`: no SPI.
pub fn refuse_prepare() {
    if crate::config::publish_changes().is_some() && !published::is_empty() {
        crate::TViewError::PrepareRefused {
            reason: crate::error::PrepareRefusal::Publish,
        }
        .raise();
    }
}

/// One message being filled, its rendered length kept exactly.
#[derive(Default)]
struct Message<'a> {
    upserted: Vec<(&'a str, Vec<&'a str>)>,
    deleted: Vec<(&'a str, Vec<&'a str>)>,
    types: BTreeSet<&'a str>,
    len: usize,
}

/// `{"v":1}`: what a message holds before anything is added.
const EMPTY_LEN: usize = r#"{"v":1}"#.len();

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// The bytes of `,"t":[…]` listing `types`.
fn types_len(types: &BTreeSet<&str>) -> usize {
    if types.is_empty() {
        return 0;
    }
    r#","t":[]"#.len() + types.iter().map(|t| json_string(t).len()).sum::<usize>() + types.len() - 1
}

impl<'a> Message<'a> {
    fn new() -> Self {
        Self {
            len: EMPTY_LEN,
            ..Self::default()
        }
    }

    /// What adding `id` of `typename` to the deleted (`deleted`) or upserted rows
    /// would add to the message.
    fn cost(&self, deleted: bool, typename: &str, id: &str) -> usize {
        let section = if deleted {
            &self.deleted
        } else {
            &self.upserted
        };
        match section.last() {
            Some((t, _)) if *t == typename => 1 + id.len(),
            Some(_) => 1 + json_string(typename).len() + 3 + id.len(),
            None => r#","u":{}"#.len() + json_string(typename).len() + 3 + id.len(),
        }
    }

    fn push(&mut self, deleted: bool, typename: &'a str, id: &'a str) {
        self.len += self.cost(deleted, typename, id);
        let section = if deleted {
            &mut self.deleted
        } else {
            &mut self.upserted
        };
        match section.last_mut() {
            Some((t, ids)) if *t == typename => ids.push(id),
            _ => section.push((typename, vec![id])),
        }
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(self.len);
        out.push_str(r#"{"v":1"#);
        for (name, rows) in [("u", &self.upserted), ("d", &self.deleted)] {
            if rows.is_empty() {
                continue;
            }
            out.push_str(",\"");
            out.push_str(name);
            out.push_str("\":{");
            for (n, (typename, ids)) in rows.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(typename));
                out.push_str(":[");
                out.push_str(&ids.join(","));
                out.push(']');
            }
            out.push('}');
        }
        if !self.types.is_empty() {
            let names: Vec<String> = self.types.iter().map(|t| json_string(t)).collect();
            out.push_str(",\"t\":[");
            out.push_str(&names.join(","));
            out.push(']');
        }
        out.push('}');
        out
    }
}

/// The messages that publish `folded`, each under [`MESSAGE_BYTES`], at most
/// [`MAX_MESSAGES`] carrying ids. Types whose ids don't fit are listed in `t` of
/// the last message; `types` mode lists every type there. Total: it never fails.
#[must_use]
pub fn encode(folded: &Folded<'_>, publish: Publish) -> Vec<String> {
    let all_types: BTreeSet<&str> = folded
        .upserted
        .iter()
        .chain(&folded.deleted)
        .map(|(t, _)| *t)
        .chain(folded.types.iter().copied())
        .collect();
    if all_types.is_empty() {
        return Vec::new();
    }
    // The last message keeps room to name every type.
    let reserve = types_len(&all_types);
    if publish == Publish::Types || reserve + EMPTY_LEN > MESSAGE_BYTES / 2 {
        return types_only(&all_types);
    }

    let mut messages = vec![Message::new()];
    let mut left: BTreeSet<&str> = folded.types.clone();
    let rows = folded
        .upserted
        .iter()
        .map(|(t, ids)| (false, *t, ids))
        .chain(folded.deleted.iter().map(|(t, ids)| (true, *t, ids)));
    'rows: for (deleted, typename, ids) in rows {
        if left.contains(typename) {
            continue;
        }
        for id in ids {
            // An id no message can carry: its type goes by name.
            if Message::new().cost(deleted, typename, id) > MESSAGE_BYTES - reserve {
                left.insert(typename);
                continue 'rows;
            }
            let count = messages.len();
            let budget = if count == MAX_MESSAGES {
                MESSAGE_BYTES - reserve
            } else {
                MESSAGE_BYTES
            };
            let fits = |m: &Message| m.len + m.cost(deleted, typename, id) <= budget;
            if !messages.last().is_some_and(fits) {
                if count == MAX_MESSAGES {
                    left.insert(typename);
                    continue 'rows;
                }
                messages.push(Message::new());
            }
            if let Some(message) = messages.last_mut() {
                message.push(deleted, typename, id);
            }
        }
    }
    // Types left over or published by name: in the last message, or one of their
    // own when the last has no room (only before the cap: the last of
    // MAX_MESSAGES kept the room).
    if !left.is_empty() {
        let fits = messages
            .last()
            .is_some_and(|m| m.len + types_len(&left) <= MESSAGE_BYTES);
        if !fits {
            messages.push(Message::new());
        }
        if let Some(message) = messages.last_mut() {
            message.len += types_len(&left);
            message.types = left;
        }
    }
    messages.iter().map(Message::render).collect()
}

/// `{"v":1,"t":[…]}` messages naming `types`, as few as fit.
fn types_only(types: &BTreeSet<&str>) -> Vec<String> {
    let mut messages = vec![Message::new()];
    for typename in types {
        let mut with = messages.last().map(|m| m.types.clone()).unwrap_or_default();
        with.insert(typename);
        if EMPTY_LEN + types_len(&with) > MESSAGE_BYTES {
            messages.push(Message::new());
        }
        if let Some(message) = messages.last_mut() {
            message.types.insert(typename);
        }
    }
    messages.iter().map(Message::render).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folded<'a>(
        upserted: &[(&'a str, &[&'a str])],
        deleted: &[(&'a str, &[&'a str])],
        types: &[&'a str],
    ) -> Folded<'a> {
        let lists =
            |l: &[(&'a str, &[&'a str])]| l.iter().map(|(t, ids)| (*t, ids.to_vec())).collect();
        Folded {
            upserted: lists(upserted),
            deleted: lists(deleted),
            types: types.iter().copied().collect(),
        }
    }

    fn json(message: &str) -> serde_json::Value {
        serde_json::from_str(message).expect("valid JSON")
    }

    #[test]
    fn one_message_of_the_v1_shape() {
        let messages = encode(
            &folded(
                &[("User", &["\"u1\"", "\"u2\""]), ("Post", &["7"])],
                &[("Comment", &["\"c1\""])],
                &["Tag"],
            ),
            Publish::Rows,
        );
        assert_eq!(
            messages,
            [r#"{"v":1,"u":{"User":["u1","u2"],"Post":[7]},"d":{"Comment":["c1"]},"t":["Tag"]}"#]
        );
    }

    #[test]
    fn empty_sections_are_left_out_and_nothing_sends_nothing() {
        assert_eq!(
            encode(&folded(&[], &[("Post", &["\"p\""])], &[]), Publish::Rows),
            [r#"{"v":1,"d":{"Post":["p"]}}"#]
        );
        assert!(encode(&Folded::default(), Publish::Rows).is_empty());
    }

    #[test]
    fn types_mode_names_every_changed_type() {
        assert_eq!(
            encode(
                &folded(&[("User", &["\"u1\""])], &[("Post", &["\"p\""])], &["Tag"]),
                Publish::Types
            ),
            [r#"{"v":1,"t":["Post","Tag","User"]}"#]
        );
    }

    #[test]
    fn adversarial_ids_and_names_stay_valid_json_under_the_bound() {
        // Ids arrive as JSON text (to_jsonb), quotes and backslashes escaped.
        let nasty: Vec<String> = (0..3000)
            .map(|i| serde_json::to_string(&format!("é\"\\{i}☃")).unwrap())
            .collect();
        let refs: Vec<&str> = nasty.iter().map(String::as_str).collect();
        let messages = encode(&folded(&[("Wé\"ird", &refs)], &[], &[]), Publish::Rows);
        for m in &messages {
            assert!(m.len() <= MESSAGE_BYTES, "{} bytes", m.len());
            json(m);
        }
    }

    #[test]
    fn an_id_too_long_for_any_message_names_its_type() {
        let long = format!("\"{}\"", "x".repeat(MESSAGE_BYTES));
        let messages = encode(
            &folded(&[("Doc", &[long.as_str()]), ("User", &["\"u\""])], &[], &[]),
            Publish::Rows,
        );
        assert_eq!(messages, [r#"{"v":1,"u":{"User":["u"]},"t":["Doc"]}"#]);
    }

    #[test]
    fn rendered_length_is_the_tracked_length() {
        let mut m = Message::new();
        m.push(false, "User", "\"a\"");
        m.push(false, "User", "\"b\"");
        m.push(false, "Post", "1");
        m.push(true, "Tag", "\"t\"");
        assert_eq!(m.render().len(), m.len);
    }

    #[test]
    fn past_eight_messages_the_rest_is_named_in_the_last() {
        let ids: Vec<String> = (0..2000)
            .map(|i| format!("\"00000000-0000-0000-0000-{i:012}\""))
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let messages = encode(
            &folded(&[("Comment", &refs), ("User", &["\"u\""])], &[], &[]),
            Publish::Rows,
        );
        assert_eq!(messages.len(), MAX_MESSAGES);
        let last = json(messages.last().unwrap());
        // Comment's remaining ids went to `t`; User's one id still fit.
        assert_eq!(last["t"], serde_json::json!(["Comment"]));
        assert_eq!(last["u"]["User"], serde_json::json!(["u"]));
        for m in &messages {
            assert!(m.len() <= MESSAGE_BYTES);
        }
    }

    #[test]
    fn types_without_room_get_a_message_of_their_own() {
        let ids: Vec<String> = (0..200)
            .map(|i| format!("\"00000000-0000-0000-0000-{i:012}\""))
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let names: Vec<String> = (0..60).map(|i| format!("Type{i:03}")).collect();
        let types: Vec<&str> = names.iter().map(String::as_str).collect();
        let messages = encode(&folded(&[("User", &refs)], &[], &types), Publish::Rows);
        let last = json(messages.last().unwrap());
        assert_eq!(last["t"].as_array().map(Vec::len), Some(60));
        for m in &messages {
            assert!(m.len() <= MESSAGE_BYTES);
        }
    }
}
