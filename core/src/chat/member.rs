use std::{num::NonZero, sync::Arc};

use crate::chat::{
    Msg,
    group::{AddMemberError, Group, InitContext, Thread},
};

/// A participant in a [`Group`].
///
/// Each `Member` has shared ownership of the messages that make up its
/// context.
#[derive(Debug, Clone)]
pub struct Member {
    /// Unique identifier of this member within its group.
    pub(super) id: MemberId,
    /// The group this member belongs to.
    pub(super) group: Arc<Group>,
    /// The thread this member reads context from. Shared with any peer added
    /// via [`Member::add_peer`].
    pub(super) thread: Arc<Thread>,
}

impl Member {
    pub(super) fn new(id: MemberId, group: Arc<Group>, thread: Arc<Thread>) -> Self {
        Self { id, group, thread }
    }

    /// Adds a peer to the group that shares this member's context thread, so
    /// both members read the same messages.
    ///
    /// Returns [`AddMemberError::AlreadyExists`] if `id` is already a member of
    /// the group. The error carries the already existing `Member`.
    pub fn add_peer(&self, id: MemberId) -> Result<Member, AddMemberError> {
        self.group.add_member_with_thread(id, &self.thread)
    }

    /// Adds a worker to the with its own separate thread, and returns it as a
    /// new member.
    ///
    /// Unlike members addded via [`Member::add_peer`], this worker will not
    /// see the same messages as this member, and vice versa. This is often
    /// useful for spawning subagents.
    ///
    /// With [`InitContext::None`], the worker starts with an empty thread.
    /// With [`InitContext::Fork`], it starts from a fork of this member's
    /// current context.
    ///
    /// Returns [`AddMemberError::AlreadyExists`] if `id` is already a member of
    /// the group. The error carries the already existing `Member`.
    pub fn add_worker(&self, id: MemberId, context: InitContext) -> Result<Member, AddMemberError> {
        let thread = match context {
            InitContext::None => Arc::new(Thread::new()),
            InitContext::Fork => self.thread.fork(),
        };
        let result = self.group.add_member_with_thread(id, &thread);
        if result.is_ok() {
            self.group.secondary.lock().unwrap().push(thread);
        }
        result
    }

    // It would be nice to iterate over references instead of cloning, but
    // that's tricky with `SkipMap`, and besides, we need to modify some
    // messages anyway. If performance matters (and it might not), the
    // lowest-hanging fruit is probably in the `Msg` enum rather than here
    // (minimize cloning of content fields, maybe with shared ownership &
    // clone-on-write logic).
    /// Returns an iterator over the messages in this member's context.
    ///
    /// Unlike the underlying thread of messages, the resulting context is
    /// relative to this member. Even when two members are sharing the same
    /// chat, the context returned by this method will be different for each
    /// member. Specifically:
    ///
    /// - Messages are filtered to include only those visible to this member.
    /// - Agents see only their own tool calls.
    /// - For messages belonging to this member, `Msg::belongs_to_first_person`
    ///   will return true, so that downstream code can format the context in
    ///   first-person perspective as needed.
    pub fn context(&self) -> impl Iterator<Item = Msg> + '_ {
        let first_person = self.id.to_first_person();
        self.thread
            .iter()
            .filter_map(move |entry| match entry.value() {
                // Messages with specific recipients are only visible to those
                // recipients; all other messages are visible to everyone.
                Msg::Direct { recipients, .. }
                | Msg::ToolResult { recipients, .. }
                | Msg::Event { recipients, .. }
                | Msg::ControlSignal { recipients, .. } => {
                    if recipients.iter().any(|r| r == &self.id) {
                        Some(entry.value().clone())
                    } else {
                        None
                    }
                }
                // For user and agent messages, set the first-person flag for
                // messages sent by this member.
                Msg::User {
                    sender,
                    timestamp,
                    content,
                } => {
                    if sender == &self.id {
                        Some(Msg::User {
                            sender: first_person.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                        })
                    } else {
                        Some(entry.value().clone())
                    }
                }
                Msg::Agent {
                    sender,
                    timestamp,
                    content,
                    tool_calls,
                    model,
                    reasoning_content,
                    metadata,
                } => {
                    if sender == &self.id {
                        Some(Msg::Agent {
                            sender: first_person.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                            tool_calls: tool_calls.clone(),
                            model: model.clone(),
                            reasoning_content: reasoning_content.clone(),
                            metadata: metadata.clone(),
                        })
                    } else {
                        Some(Msg::Agent {
                            sender: sender.clone(),
                            timestamp: timestamp.clone(),
                            content: content.clone(),
                            tool_calls: if !self.human() {
                                // Hide tool calls from other agents
                                Vec::new()
                            } else {
                                // But not from humans
                                tool_calls.clone()
                            },
                            model: model.clone(),
                            reasoning_content: reasoning_content.clone(),
                            metadata: metadata.clone(),
                        })
                    }
                }
                // General instructions are yielded as-is.
                _ => Some(entry.value().clone()),
            })
    }

    /// The group this member belongs to.
    pub fn group(&self) -> &Arc<Group> {
        &self.group
    }

    /// Whether this member is a human. See [`MemberId::human`].
    pub fn human(&self) -> bool {
        self.id.human()
    }

    /// The identifier of this member within its group.
    pub fn id(&self) -> &MemberId {
        &self.id
    }

    /// The thread this member reads context from.
    ///
    /// Members that share a thread (e.g. via [`Member::add_peer`]) return
    /// equal references, so this can be used to check whether two members
    /// see the same messages.
    pub fn thread(&self) -> &Thread {
        &self.thread
    }
}

/// Identifies a member within a [`Group`].
#[derive(Clone)]
pub struct MemberId {
    repr: Repr,
}

#[derive(Clone)]
enum Repr {
    Inline {
        text: [u8; INLINE_CAP],
        meta: NonZero<u8>,
    },
    Boxed(Box<BoxedText>),
}

#[derive(Clone)]
struct BoxedText {
    split: usize,
    flags: u8,
    text: Box<str>,
}

// Text (name+id, or id alone when the name is a prefix of the id) is stored
// inline when it fits in `INLINE_CAP` bytes. The inline text is zero-padded,
// and inline text never contains a NUL byte, so its length is the position of
// the first NUL (or `INLINE_CAP` if there is none).
//
// `meta` packs the split position and flags into one nonzero byte, which also
// lets the enum use a niche to fit in 16 bytes.
const INLINE_CAP: usize = 15;
const SENTINEL_BIT: u8 = 0b1000_0000;
const OVERLAP_FLAG: u8 = 0b0100_0000;
const SPLIT_SHIFT: u32 = 2;
const SPLIT_MASK: u8 = 0b1111;
const HUMAN_FLAG: u8 = 0b01;
const FIRST_PERSON_FLAG: u8 = 0b10;
const FLAGS_MASK: u8 = HUMAN_FLAG | FIRST_PERSON_FLAG | OVERLAP_FLAG;

impl MemberId {
    /// Creates an id with no display name.
    ///
    /// `human` marks the member as a human rather than an AI agent.
    pub fn new(id: &str, human: bool) -> Self {
        Self::new_internal(id, None, human)
    }

    /// Creates an id with a display name, in addition to the id itself.
    ///
    /// If `name` is an empty string, no name is added and
    /// [`MemberId::display_name`] will return `None`.
    ///
    /// If a name is given, this value will be used for the `Display` trait
    /// instead of [`MemberId::id`].
    ///
    /// This value does not affect equality or hashing.
    pub fn with_display_name(id: &str, name: &str, human: bool) -> Self {
        Self::new_internal(id, Some(name), human)
    }

    fn new_internal(id: &str, name: Option<&str>, human: bool) -> Self {
        let flags = if human { HUMAN_FLAG } else { 0 };
        let split = name.map_or(0, str::len);
        let overlap = name.is_some_and(|n| id.starts_with(n));
        let flags = flags | if overlap { OVERLAP_FLAG } else { 0 };
        // Parts of the stored text. When the name is a prefix of the id, only
        // the id is stored, and the name shares its bytes.
        let parts: [&str; 2] = match name {
            Some(n) if !overlap => [n, id],
            _ => ["", id],
        };
        let total = parts[0].len() + parts[1].len();
        let has_nul = parts.iter().any(|p| p.as_bytes().contains(&0));

        let repr = if split <= SPLIT_MASK as usize && total <= INLINE_CAP && !has_nul {
            let mut text = [0u8; INLINE_CAP];
            let mid = parts[0].len();
            text[..mid].copy_from_slice(parts[0].as_bytes());
            text[mid..total].copy_from_slice(parts[1].as_bytes());
            let meta = SENTINEL_BIT | ((split as u8) << SPLIT_SHIFT) | flags;
            Repr::Inline {
                text,
                meta: NonZero::new(meta).expect("sentinel bit is always set"),
            }
        } else {
            let mut text = String::with_capacity(total);
            text.push_str(parts[0]);
            text.push_str(parts[1]);
            Repr::Boxed(Box::new(BoxedText {
                split,
                flags,
                text: text.into_boxed_str(),
            }))
        };
        Self { repr }
    }

    /// Returns the stored text, the split position, and the flag bits.
    fn parts(&self) -> (&str, usize, u8) {
        match &self.repr {
            Repr::Inline { text, meta } => {
                let len = text.iter().position(|&b| b == 0).unwrap_or(INLINE_CAP);
                let s = str::from_utf8(&text[..len])
                    .expect("MemberId text is valid UTF-8 by construction");
                let split = ((meta.get() >> SPLIT_SHIFT) & SPLIT_MASK) as usize;
                (s, split, meta.get() & FLAGS_MASK)
            }
            Repr::Boxed(b) => (&b.text, b.split, b.flags),
        }
    }

    /// Returns a copy of this id flagged as the first-person subject.
    fn to_first_person(&self) -> Self {
        let mut out = self.clone();
        match &mut out.repr {
            Repr::Inline { meta, .. } => {
                *meta = NonZero::new(meta.get() | FIRST_PERSON_FLAG)
                    .expect("sentinel bit is always set");
            }
            Repr::Boxed(b) => b.flags |= FIRST_PERSON_FLAG,
        }
        out
    }

    /// The display name, if one was given when this id was created.
    pub fn display_name(&self) -> Option<&str> {
        let (text, split, _) = self.parts();
        if split == 0 {
            None
        } else {
            Some(&text[..split])
        }
    }

    /// The member identifier, which is unique within its group.
    ///
    /// This is the value used for equality and hashing.
    pub fn id(&self) -> &str {
        let (text, split, flags) = self.parts();
        if flags & OVERLAP_FLAG != 0 {
            text
        } else {
            &text[split..]
        }
    }

    /// Whether this member represents a human as opposed to an AI agent.
    ///
    /// This value does not affect equality or hashing since groups cannot
    /// contain multiple members with the same id but different `human` flags
    /// (e.g. a human and an AI agent both named "Alice" would clash).
    pub fn human(&self) -> bool {
        self.parts().2 & HUMAN_FLAG != 0
    }

    /// Value used to flag the first-person subject in a specific context.
    /// This is only relevant within a chat context, and even then it is
    /// typically not used directly but via `Msg::is_first_person()`.
    ///
    /// This value does not affect equality or hashing.
    pub fn is_first_person(&self) -> bool {
        self.parts().2 & FIRST_PERSON_FLAG != 0
    }
}

impl std::fmt::Debug for MemberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberId")
            .field("display_name", &self.display_name())
            .field("id", &self.id())
            .field("human", &self.human())
            .finish()
    }
}

impl std::fmt::Display for MemberId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name().unwrap_or_else(|| self.id()))
    }
}

// Enables looking members up by their raw string representation, e.g. as
// found in `Msg::User::sender`, without allocating a `MemberId`.
impl std::borrow::Borrow<str> for MemberId {
    fn borrow(&self) -> &str {
        self.id()
    }
}

// Equality and hashing are based on `id` alone (never `name` or `split`), so
// that two `MemberId`s are never considered equal unless their ids actually
// match -- e.g. name = "Al", id = "ice123" must not equal name = "", id =
// "Alice123" just because their underlying text happens to coincide.
impl PartialEq for MemberId {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for MemberId {}

impl std::hash::Hash for MemberId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn members_share_thread_only_with_peers() {
        let group = Group::new();
        let alice = group.add_member(MemberId::new("Alice", true)).unwrap();
        let bob = alice.add_peer(MemberId::new("Bob", true)).unwrap();
        let worker = alice
            .add_worker(MemberId::new("Carol", false), InitContext::None)
            .unwrap();
        let forked = alice
            .add_worker(MemberId::new("Dave", false), InitContext::Fork)
            .unwrap();

        assert_eq!(alice.thread(), bob.thread());
        assert_ne!(alice.thread(), worker.thread());
        assert_ne!(alice.thread(), forked.thread());
        assert_ne!(worker.thread(), forked.thread());

        let set: HashSet<&Thread> = [alice.thread(), bob.thread(), worker.thread()]
            .into_iter()
            .collect();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn member_id_basics() {
        let id = MemberId::new("Alice123", true);
        assert_eq!(id.id(), "Alice123");
        assert_eq!(id.display_name(), None);
        assert!(id.human());

        let with_name = MemberId::with_display_name("Alice123", "Alice", true);
        assert_eq!(with_name.id(), "Alice123");
        assert_eq!(with_name.display_name(), Some("Alice"));
        assert!(with_name.human());

        let ai_id = MemberId::new("Robbie", false).to_first_person();
        assert!(!ai_id.human());
        assert!(ai_id.is_first_person());

        let non_ascii_id = MemberId::new("葛飾", true);
        assert_eq!(non_ascii_id.id(), "葛飾");
    }

    #[test]
    fn excessively_long_names() {
        // > 16 bytes
        let long_name = "Hippocrates of Kos";
        assert_eq!(long_name.len(), 18);
        let long = MemberId::with_display_name("h2", long_name, true);
        assert_eq!(long.id(), "h2");
        assert_eq!(long.display_name(), Some(long_name));

        // > 256 bytes
        let longer_name = "Hippocrates of Kos, the Father of Medicine 👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️👨🏽‍⚕️";
        assert_eq!(longer_name.len(), 264);
        let longer = MemberId::with_display_name("h2", longer_name, true);
        assert_eq!(longer.id(), "h2");
        assert_eq!(longer.display_name(), Some(longer_name));
    }

    #[test]
    fn member_id_equality_depends_only_on_id() {
        // Same id, different flags: still equal.
        let a = MemberId::new("Alice123", true);
        let b = MemberId::new("Alice123", false);
        let c = MemberId::new("Alice123", true);
        let d = MemberId::new("Alice123", false);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);

        // Different ids must never compare equal, even when a name makes their
        // stored text coincide: name "Al" + id "ice123" stores "Alice123".
        let e = MemberId::with_display_name("ice123", "Al", true);
        assert_eq!(e.id(), "ice123");
        assert_ne!(e, a);

        // Names do not affect equality.
        let named = MemberId::with_display_name("Alice123", "Bob", false);
        assert_eq!(named, a);

        let c = MemberId::new("Alice124", true);
        assert_ne!(a, c);

        // Usable as a HashMap key, looked up by raw `id`.
        let mut map = HashMap::new();
        map.insert(a.clone(), "first");
        assert_eq!(map.get(&b), Some(&"first"));
        assert_eq!(map.get(&c), None);
    }

    #[test]
    fn member_id_inline_or_boxed() {
        assert_eq!(std::mem::size_of::<MemberId>(), 16);
        let is_inline = |id: &MemberId| matches!(id.repr, Repr::Inline { .. });

        // Up to 15 bytes of stored text is inline.
        assert!(is_inline(&MemberId::new("abcdefghijklmno", true)));
        assert!(!is_inline(&MemberId::new("abcdefghijklmnop", true)));

        // Stored text is name + id, unless the name is a prefix of the id.
        assert!(is_inline(&MemberId::with_display_name(
            "Alice123", "Alice", true
        )));
        assert!(is_inline(&MemberId::with_display_name(
            "Alice1234567890",
            "Alice",
            true
        )));
        assert!(!is_inline(&MemberId::with_display_name(
            "Alice123456789",
            "Bobbyyy",
            true
        )));

        // Split position must fit in 4 bits, so a longer name is boxed.
        assert!(!is_inline(&MemberId::with_display_name(
            "Alice",
            "abcdefghijklmnop",
            true
        )));

        // NUL bytes would clash with the zero padding used for inline text.
        let nul = MemberId::new("a\0b", true);
        assert!(!is_inline(&nul));
        assert_eq!(nul.id(), "a\0b");
    }

    #[test]
    fn empty_display_name_is_treated_as_no_name() {
        let empty_name = MemberId::with_display_name("Alice123", "", true);
        assert_eq!(empty_name.display_name(), None);
        assert_eq!(empty_name.id(), "Alice123");
        assert_eq!(empty_name.to_string(), "Alice123");
        assert_eq!(empty_name, MemberId::new("Alice123", true));
        assert!(empty_name.human());
    }

    #[test]
    fn member_id_name_prefix_shares_storage() {
        let addr = |s: &str| s.as_ptr() as usize;
        let within = |inner: &str, outer: &str| {
            addr(inner) >= addr(outer) && addr(inner) + inner.len() <= addr(outer) + outer.len()
        };

        // Name is a prefix of the id: the display name borrows from the id.
        let overlap = MemberId::with_display_name("Alice1234567890", "Alice", true);
        assert_eq!(overlap.id(), "Alice1234567890");
        assert_eq!(overlap.display_name(), Some("Alice"));
        assert_eq!(addr(overlap.display_name().unwrap()), addr(overlap.id()));
        assert!(within(overlap.display_name().unwrap(), overlap.id()));

        // Name is not a prefix: the name and id occupy separate bytes.
        let separate = MemberId::with_display_name("Alice123", "Bob", true);
        assert_eq!(separate.id(), "Alice123");
        assert_eq!(separate.display_name(), Some("Bob"));
        assert!(!within(separate.display_name().unwrap(), separate.id()));
        assert!(!within(separate.id(), separate.display_name().unwrap()));

        // The same holds for boxed text.
        let long = MemberId::with_display_name(
            "abcdefghijklmnopqrstuvwxyz",
            "abcdefghijklmnopqrstuvw",
            false,
        );
        assert_eq!(long.id(), "abcdefghijklmnopqrstuvwxyz");
        assert_eq!(long.display_name(), Some("abcdefghijklmnopqrstuvw"));
        assert!(within(long.display_name().unwrap(), long.id()));
        assert!(!long.human());
    }
}
