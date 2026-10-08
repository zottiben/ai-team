//! Does this whole message instruct a push?
//!
//! The parse is deliberately small and closed. Every word of the message has to be in a
//! fixed vocabulary and the whole sequence has to parse as an instruction - so a message
//! that quotes, negates, conditions, reports or pastes the same words fails, because the
//! words around them are not in the vocabulary. Nothing here does substring matching:
//! "do not push" and "they pushed it" are whole-word parses that end in a refusal, not a
//! search for "push" inside a sentence.
//!
//! A failed parse is never authority. At most it is `Ambiguous`, which asks the person to
//! say it exactly rather than guessing on their behalf.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Nothing about publishing. The chat behaves as it always has.
    None,
    /// Publishing was mentioned but not instructed. No authority is issued.
    Ambiguous,
    /// An explicit instruction. `commit` is true when the person also asked for the
    /// commit, which is what lets the agent pin a commit it has not made yet.
    Authorize { commit: bool },
}

/// Longer than this is a paste, a specification or a quotation, not an instruction.
const MAX_CHARS: usize = 160;
const MAX_WORDS: usize = 24;

/// The two imperatives. Only the bare imperative form: "pushed" and "pushing" report
/// something, they do not ask for it.
const PUSH: &[&str] = &["push"];
const COMMIT: &[&str] = &["commit"];

/// Words that join clauses.
const CONJ: &[&str] = &["and", "then", "also", "plus"];

/// Words that may sit inside an instruction clause without changing what it asks for:
/// politeness, and the objects an imperative can take. Anything naming a destination,
/// another branch, a PR, a tag or a force is deliberately absent, so a message that names
/// one fails to parse and is asked about instead.
const FILLER: &[&str] = &[
    "a",
    "ahead",
    "all",
    "both",
    "branch",
    "change",
    "changes",
    "code",
    "current",
    "edits",
    "everything",
    "for",
    "go",
    "it",
    "just",
    "me",
    "my",
    "now",
    "of",
    "ok",
    "okay",
    "our",
    "please",
    "stuff",
    "thanks",
    "that",
    "the",
    "them",
    "these",
    "this",
    "those",
    "to",
    "up",
    "us",
    "work",
    "you",
    "your",
];

/// Words that start an aside about what the person will do themselves.
const SUBJECT: &[&str] = &[
    "i", "ill", "i'll", "im", "i'm", "we", "we'll", "weve", "we've",
];

/// What an aside may contain. An imperative inside one is not an instruction to the
/// agent - "ill push it later" is the person saying they will - so the parse refuses it
/// rather than reading the verb out of its clause.
const ASIDE: &[&str] = &[
    "a",
    "after",
    "am",
    "and",
    "care",
    "create",
    "creating",
    "do",
    "draft",
    "from",
    "going",
    "gonna",
    "handle",
    "here",
    "i",
    "i'll",
    "i'm",
    "ill",
    "im",
    "it",
    "later",
    "make",
    "making",
    "merge",
    "myself",
    "of",
    "on",
    "open",
    "opening",
    "ourselves",
    "pr",
    "prs",
    "pull",
    "raise",
    "raising",
    "request",
    "rest",
    "review",
    "take",
    "thank",
    "thanks",
    "that",
    "the",
    "then",
    "up",
    "we",
    "we'll",
    "will",
    "you",
];

/// Whole words that mention publishing. These alone cannot distinguish a delivery
/// request from an ordinary task about publishing functionality.
const MENTION: &[&str] = &[
    "push",
    "pushed",
    "pushes",
    "pushing",
    "publish",
    "published",
    "publishes",
    "publishing",
];

pub fn classify(message: &str) -> Intent {
    // One keyboard's right single quote is another's apostrophe. Everything else outside
    // the allowed set stays outside it.
    let normalized = message.replace('\u{2019}', "'");
    let text = normalized.trim();
    // A negative constraint remains an ordinary task instruction, with no grant. This
    // scan can only withhold authority; granting still requires the whole-message parse.
    let tokens: Vec<_> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if tokens
        .windows(2)
        .any(|w| matches!(w[0].as_str(), "not" | "never" | "don't" | "dont") && w[1] == "push")
        || tokens.windows(4).any(|w| {
            matches!(w[0].as_str(), "not" | "never" | "don't" | "dont")
                && w[1] == "commit"
                && w[2] == "or"
                && w[3] == "push"
        })
    {
        return Intent::None;
    }
    match words(text).as_deref().and_then(parse) {
        Some((true, commit)) => Intent::Authorize { commit },
        Some((false, _)) => Intent::None,
        None => mention(text),
    }
}

/// The message, as lower-case words - or `None` when a size or character guard refuses it.
///
/// The character guard is what keeps quotations, code, paths, URLs, blockquoted review
/// text and anything multi-line out: none of them can be written with letters, digits and
/// four pieces of sentence punctuation.
fn words(text: &str) -> Option<Vec<String>> {
    if text.is_empty() || text.chars().count() > MAX_CHARS {
        return None;
    }
    if !text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '\'' | ',' | '.' | '!'))
    {
        return None;
    }
    let found: Vec<String> = text
        .split(' ')
        .map(|raw| raw.trim_matches(|c| matches!(c, ',' | '.' | '!' | '-')))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    (!found.is_empty() && found.len() <= MAX_WORDS).then_some(found)
}

enum State {
    Clause,
    Aside,
}

/// `(push, commit)`, or `None` when the message is not wholly an instruction.
fn parse(words: &[String]) -> Option<(bool, bool)> {
    let mut start = 0;
    while words
        .get(start)
        .is_some_and(|w| matches!(w.as_str(), "ok" | "okay" | "please"))
    {
        start += 1;
    }
    for prefix in [
        &["go", "ahead", "and"][..],
        &["can", "you"],
        &["could", "you"],
        &["would", "you"],
    ] {
        if words
            .get(start..start + prefix.len())
            .is_some_and(|w| w.iter().map(String::as_str).eq(prefix.iter().copied()))
        {
            start += prefix.len();
            break;
        }
    }
    if words.get(start).is_some_and(|w| w == "please") {
        start += 1;
    }
    if !words
        .get(start)
        .is_some_and(|w| PUSH.contains(&w.as_str()) || COMMIT.contains(&w.as_str()))
    {
        return None;
    }
    let mut state = State::Clause;
    let (mut push, mut commit) = (false, false);
    for (index, word) in words.iter().enumerate().skip(start) {
        let word = word.as_str();
        match state {
            State::Clause => {
                if PUSH.contains(&word) || COMMIT.contains(&word) {
                    if index != start && !CONJ.contains(&words[index - 1].as_str()) {
                        return None;
                    }
                    push |= PUSH.contains(&word);
                    commit |= COMMIT.contains(&word);
                } else if SUBJECT.contains(&word) {
                    state = State::Aside;
                } else if !FILLER.contains(&word) && !CONJ.contains(&word) {
                    return None;
                }
            }
            State::Aside => {
                if PUSH.contains(&word) || COMMIT.contains(&word) || !ASIDE.contains(&word) {
                    return None;
                }
            }
        }
    }
    Some((push, commit))
}

fn mention(text: &str) -> Intent {
    let tokens: Vec<_> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mentioned = tokens.iter().any(|word| MENTION.contains(&word.as_str()));
    // Only an apparent delivery request needs clarification instead of team planning.
    // This test can withhold authority, never grant it: the whole-message parser above
    // still owns that decision. "Add a Publish button" is ordinary coding work.
    let head = tokens.iter().find(|word| {
        !matches!(
            word.as_str(),
            "can"
                | "could"
                | "would"
                | "do"
                | "did"
                | "have"
                | "will"
                | "should"
                | "you"
                | "we"
                | "please"
                | "ok"
                | "okay"
                | "go"
                | "ahead"
                | "and"
                | "just"
        )
    });
    let request = head.is_some_and(|word| {
        matches!(
            word.as_str(),
            "push"
                | "publish"
                | "commit"
                | "force"
                | "git"
                | "if"
                | "when"
                | "once"
                | "after"
                | "unless"
        )
    });
    if mentioned && request {
        Intent::Ambiguous
    } else {
        Intent::None
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, Intent};

    const PUSH_ONLY: Intent = Intent::Authorize { commit: false };
    const PUSH_AND_COMMIT: Intent = Intent::Authorize { commit: true };

    #[test]
    fn the_plain_instruction_authorises_and_its_aside_does_not_change_it() {
        assert_eq!(
            classify("commit and push and ill open the PR"),
            PUSH_AND_COMMIT
        );
        assert_eq!(
            classify("commit and push and I'll open the PR"),
            PUSH_AND_COMMIT
        );
        assert_eq!(
            classify("Commit and push, then I'll open the PR."),
            PUSH_AND_COMMIT
        );
        assert_eq!(classify("push"), PUSH_ONLY);
        assert_eq!(classify("please push it up"), PUSH_ONLY);
        assert_eq!(classify("can you push the changes"), PUSH_ONLY);
        assert_eq!(
            classify("go ahead and commit and push everything"),
            PUSH_AND_COMMIT
        );
    }

    #[test]
    fn ordinary_tasks_can_discuss_publishing_without_becoming_delivery_requests() {
        for message in [
            "Add a Publish button to the settings page and wire it to the export service.",
            "Explain how git push works.",
            "Can you implement push notifications in the mobile app?",
            "Update the publishing documentation. Include the release checklist, all the supported platforms, troubleshooting for interrupted uploads, and a clear description of the review process before anything is published.",
        ] {
            assert_eq!(classify(message), Intent::None, "{message}");
        }
        assert_eq!(classify("can you push it?"), Intent::Ambiguous);
    }

    #[test]
    fn commit_alone_is_not_push_authority() {
        assert_eq!(classify("commit it"), Intent::None);
        assert_eq!(classify("commit the changes please"), Intent::None);
    }

    #[test]
    fn quoted_reported_negated_and_conditional_text_never_authorises() {
        for message in [
            "our branch can push",
            "you would push",
            "a commit and push",
            "- push",
            "push only if the tests pass",
            "when the tests pass, push",
            "the house rules say \"commit and push\"",
            "they said: commit and push and ill open the PR",
            "for example, commit and push and ill open the PR",
            "the reviewer asked whether we should commit and push",
            "ill push it later",
            "I'll commit and push myself",
            "we pushed it already",
            "why did you push",
            "`commit and push`",
            "commit and push and ill open the PR\nalso delete the branch",
        ] {
            assert!(
                !matches!(classify(message), Intent::Authorize { .. }),
                "{message}"
            );
        }
    }

    #[test]
    fn negative_constraints_do_not_turn_work_into_a_publication_request() {
        for message in [
            "do not push",
            "don't push this",
            "never push to main",
            "Fix the bug; do not commit or push",
        ] {
            assert_eq!(classify(message), Intent::None, "{message}");
        }
    }

    #[test]
    fn a_named_destination_pr_or_force_is_asked_about_rather_than_guessed() {
        for message in [
            "push to origin main",
            "push to main",
            "force push",
            "push and open the PR",
            "push and tag it",
            "push the pr",
            "commit and push and merge it",
        ] {
            assert_eq!(classify(message), Intent::Ambiguous, "{message}");
        }
    }

    #[test]
    fn ordinary_work_is_untouched() {
        for message in [
            "add a test for the parser",
            "what does this function do",
            "",
            "   ",
        ] {
            assert_eq!(classify(message), Intent::None, "{message}");
        }
    }

    #[test]
    fn a_message_too_long_to_be_an_instruction_is_asked_about() {
        let pasted = "commit and push and ill open the PR ".repeat(12);
        assert_eq!(classify(&pasted), Intent::Ambiguous);
    }
}
