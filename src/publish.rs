//! Publishing a committed transaction's changed TVIEW rows on `tviews_changes`
//! (ADR 0231).
//!
//! The transaction callback calls [`at_pre_commit`]. It encodes the
//! transaction's publish set ([`crate::queue::published`]) into messages of one
//! shape, version 2:
//!
//! ```json
//! {"v":2,"u":{"User":["8f…"]},"d":{"Comment":["1c…"]},"c":{"Comment.fk_post":["9a…"]},"t":["Tag"]}
//! ```
//!
//! `u` lists the rows inserted or updated by type, `d` the rows deleted, `c` the
//! parents whose collections of child rows changed, by collection key (the child
//! type and its reference column), `t` the types whose rows and collections are
//! not listed. Nothing here may raise: an error at
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
    /// The committing transaction skipped publishing: [`at_commit`] counts it.
    static GAP: Cell<bool> = const { Cell::new(false) };
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
        GAP.set(true);
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

/// The transaction committed: if it skipped publishing, count the gap for every
/// listener ([`crate::stats::add_feed_gap`]). Counted now, not at `PRE_COMMIT`:
/// a listener evicting on the counter must find the transaction's rows visible.
pub fn at_commit() {
    if GAP.replace(false) {
        crate::stats::add_feed_gap();
    }
}

/// The transaction ended without committing: no gap.
pub fn forget_gap() {
    GAP.set(false);
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

/// The sections of a message that list ids, in the order they are rendered:
/// rows upserted and deleted by type, then collections by key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Upserted,
    Deleted,
    Collections,
}

impl Section {
    const ALL: [Self; 3] = [Self::Upserted, Self::Deleted, Self::Collections];

    const fn name(self) -> &'static str {
        match self {
            Self::Upserted => "u",
            Self::Deleted => "d",
            Self::Collections => "c",
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// One message being filled, its rendered length kept exactly.
#[derive(Default)]
struct Message<'a> {
    /// Per [`Section`], `(type or collection key, ids)`.
    sections: [Vec<(&'a str, Vec<&'a str>)>; 3],
    types: BTreeSet<&'a str>,
    len: usize,
}

/// `{"v":2}`: what a message holds before anything is added.
const EMPTY_LEN: usize = r#"{"v":2}"#.len();

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

    /// What adding `id` under `name` (a type, or a collection key) to `section`
    /// would add to the message.
    fn cost(&self, section: Section, name: &str, id: &str) -> usize {
        match self.sections[section.index()].last() {
            Some((n, _)) if *n == name => 1 + id.len(),
            Some(_) => 1 + json_string(name).len() + 3 + id.len(),
            None => r#","u":{}"#.len() + json_string(name).len() + 3 + id.len(),
        }
    }

    fn push(&mut self, section: Section, name: &'a str, id: &'a str) {
        self.len += self.cost(section, name, id);
        let lists = &mut self.sections[section.index()];
        match lists.last_mut() {
            Some((n, ids)) if *n == name => ids.push(id),
            _ => lists.push((name, vec![id])),
        }
    }

    fn render(&self) -> String {
        let mut out = String::with_capacity(self.len);
        out.push_str(r#"{"v":2"#);
        for section in Section::ALL {
            let lists = &self.sections[section.index()];
            if lists.is_empty() {
                continue;
            }
            out.push_str(",\"");
            out.push_str(section.name());
            out.push_str("\":{");
            for (n, (name, ids)) in lists.iter().enumerate() {
                if n > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(name));
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
/// the last message, and so is the child type of a collection whose parent ids
/// don't fit: `t` evicts its rows and its collections. `types` mode lists every
/// type there. Total: it never fails.
#[must_use]
pub fn encode(folded: &Folded<'_>, publish: Publish) -> Vec<String> {
    let all_types: BTreeSet<&str> = folded
        .upserted
        .iter()
        .chain(&folded.deleted)
        .map(|(t, _)| *t)
        .chain(folded.types.iter().copied())
        .chain(folded.collections.iter().map(|(_, t, _)| *t))
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
    // (section, name, the type it falls back to, ids)
    let lists = folded
        .upserted
        .iter()
        .map(|(t, ids)| (Section::Upserted, *t, *t, ids))
        .chain(
            folded
                .deleted
                .iter()
                .map(|(t, ids)| (Section::Deleted, *t, *t, ids)),
        )
        .chain(
            folded
                .collections
                .iter()
                .map(|(key, t, ids)| (Section::Collections, *key, *t, ids)),
        );
    'lists: for (section, name, typename, ids) in lists {
        if left.contains(typename) {
            continue;
        }
        for id in ids {
            // An id no message can carry: its type goes by name.
            if Message::new().cost(section, name, id) > MESSAGE_BYTES - reserve {
                left.insert(typename);
                continue 'lists;
            }
            let count = messages.len();
            let budget = if count == MAX_MESSAGES {
                MESSAGE_BYTES - reserve
            } else {
                MESSAGE_BYTES
            };
            let fits = |m: &Message| m.len + m.cost(section, name, id) <= budget;
            if !messages.last().is_some_and(fits) {
                if count == MAX_MESSAGES {
                    left.insert(typename);
                    continue 'lists;
                }
                messages.push(Message::new());
            }
            if let Some(message) = messages.last_mut() {
                message.push(section, name, id);
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

/// `{"v":2,"t":[…]}` messages naming `types`, as few as fit.
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
            collections: Vec::new(),
        }
    }

    fn with_collections<'a>(
        mut f: Folded<'a>,
        collections: &[(&'a str, &'a str, &[&'a str])],
    ) -> Folded<'a> {
        f.collections = collections
            .iter()
            .map(|(k, t, ids)| (*k, *t, ids.to_vec()))
            .collect();
        f
    }

    fn json(message: &str) -> serde_json::Value {
        serde_json::from_str(message).expect("valid JSON")
    }

    #[test]
    fn one_message_of_the_v2_shape() {
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
            [r#"{"v":2,"u":{"User":["u1","u2"],"Post":[7]},"d":{"Comment":["c1"]},"t":["Tag"]}"#]
        );
    }

    #[test]
    fn empty_sections_are_left_out_and_nothing_sends_nothing() {
        assert_eq!(
            encode(&folded(&[], &[("Post", &["\"p\""])], &[]), Publish::Rows),
            [r#"{"v":2,"d":{"Post":["p"]}}"#]
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
            [r#"{"v":2,"t":["Post","Tag","User"]}"#]
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
        assert_eq!(messages, [r#"{"v":2,"u":{"User":["u"]},"t":["Doc"]}"#]);
    }

    #[test]
    fn rendered_length_is_the_tracked_length() {
        let mut m = Message::new();
        m.push(Section::Upserted, "User", "\"a\"");
        m.push(Section::Upserted, "User", "\"b\"");
        m.push(Section::Upserted, "Post", "1");
        m.push(Section::Deleted, "Tag", "\"t\"");
        m.push(Section::Collections, "Tag.fk_post", "\"p\"");
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

    #[test]
    fn collections_follow_the_rows_in_c() {
        let messages = encode(
            &with_collections(
                folded(&[("Comment", &["\"c1\""])], &[], &[]),
                &[
                    ("Comment.fk_author", "Comment", &["\"u1\""]),
                    ("Comment.fk_post", "Comment", &["\"p1\"", "\"p2\""]),
                ],
            ),
            Publish::Rows,
        );
        assert_eq!(
            messages,
            [
                r#"{"v":2,"u":{"Comment":["c1"]},"c":{"Comment.fk_author":["u1"],"Comment.fk_post":["p1","p2"]}}"#
            ]
        );
    }

    #[test]
    fn a_collection_past_the_cap_names_its_child_type() {
        let ids: Vec<String> = (0..2000)
            .map(|i| format!("\"00000000-0000-0000-0000-{i:012}\""))
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let messages = encode(
            &with_collections(
                folded(&[("Comment", &["\"c1\""])], &[], &[]),
                &[("Comment.fk_post", "Comment", &refs)],
            ),
            Publish::Rows,
        );
        assert_eq!(messages.len(), MAX_MESSAGES);
        let last = json(messages.last().unwrap());
        assert_eq!(last["t"], serde_json::json!(["Comment"]));
        for m in &messages {
            assert!(m.len() <= MESSAGE_BYTES, "{} bytes", m.len());
            assert_eq!(json(m)["v"], 2);
        }
    }

    #[test]
    fn a_child_type_named_drops_its_collections() {
        let messages = encode(
            &with_collections(
                folded(&[], &[], &["Comment"]),
                &[("Comment.fk_post", "Comment", &["\"p1\""])],
            ),
            Publish::Rows,
        );
        assert_eq!(messages, [r#"{"v":2,"t":["Comment"]}"#]);
    }

    #[test]
    fn types_mode_publishes_no_collection() {
        let messages = encode(
            &with_collections(
                folded(&[("Comment", &["\"c1\""])], &[], &[]),
                &[("Comment.fk_post", "Comment", &["\"p1\""])],
            ),
            Publish::Types,
        );
        assert_eq!(messages, [r#"{"v":2,"t":["Comment"]}"#]);
    }
}
