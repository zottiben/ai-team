//! Explicit direct push clauses can accompany ordinary task instructions. Approval is
//! never inferred from a keyword in quoted, reported, conditional or negative text.

mod clauses;
pub(crate) use clauses::{read as request, Request};

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
    "again",
    "ahead",
    "all",
    "already",
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
    "too",
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
    request(message).intent
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
    if tokens.windows(2).any(|pair| {
        MENTION.contains(&pair[0].as_str())
            && matches!(
                pair[1].as_str(),
                "notifications"
                    | "notification"
                    | "button"
                    | "buttons"
                    | "step"
                    | "steps"
                    | "handler"
                    | "service"
                    | "fails"
                    | "failed"
            )
    }) {
        return Intent::None;
    }
    let mentioned = tokens.iter().any(|word| MENTION.contains(&word.as_str()));
    // Only an apparent delivery request needs clarification instead of team planning.
    // This test can withhold authority, never grant it: direct instruction parsing owns
    // that decision. "Add a Publish button" is ordinary coding work.
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
    fn direct_approval_is_not_lost_inside_a_branch_task_or_natural_request() {
        for message in [
            "create and new branch off of latest master and name is appropriately. The commit and push and I’ll open the PR.",
            "Create a new branch from main. Commit and push it; I'll open the PR.",
            "Fix the bug, then commit and push the changes. I'll open the PR.",
            "Create a new branch from main and commit and push it.",
            "Fix the flaky test but do not change the snapshot. Then commit and push and I'll open the PR.",
        ] {
            assert_eq!(classify(message), PUSH_AND_COMMIT, "{message}");
        }
        for message in [
            "I want the branch pushed",
            "I'd like you to push the branch please",
            "Can you push it?",
            "Please push the branch draft",
            "The checks passed. Push the branch please.",
            "Create a new branch from main and push it.",
            "No PRs please. Push the branch.",
            "Push the branch again",
            "Push the branch too",
            "Great, push it please",
            "This looks good. Please push the branch.",
            "Looks good, and push the branch",
            "Push the branch, no PR please.",
            "Push it, don't open a PR.",
        ] {
            assert_eq!(classify(message), PUSH_ONLY, "{message}");
        }
    }

    #[test]
    fn branch_scope_distinguishes_names_from_adverbs_and_keeps_case() {
        assert_eq!(super::request("push the branch again").branch, None);
        assert_eq!(
            super::request("Please push the branch draft")
                .branch
                .as_deref(),
            Some("draft")
        );
        assert_eq!(super::request("push the branch too").branch, None);
        assert_eq!(
            super::request("push the branch Feature/Fix")
                .branch
                .as_deref(),
            Some("Feature/Fix")
        );
        assert_eq!(
            super::request("push the branch named work")
                .branch
                .as_deref(),
            Some("work")
        );
        let request = super::request("Create a new branch from main and commit and push it.");
        let new = request
            .new_branch
            .expect("approval retains the requested creation scope");
        assert_eq!(new.base.as_deref(), Some("main"));
        assert!(new.name.is_none());
    }

    #[test]
    fn ordinary_tasks_can_discuss_publishing_without_becoming_delivery_requests() {
        for message in [
            "Add a Publish button to the settings page and wire it to the export service.",
            "Explain how git push works.",
            "Can you implement push notifications in the mobile app?",
            "When the publish step fails, retry it.",
            "Push notifications are broken; fix the retry logic.",
            "Update the publishing documentation. Include the release checklist, all the supported platforms, troubleshooting for interrupted uploads, and a clear description of the review process before anything is published.",
        ] {
            assert_eq!(classify(message), Intent::None, "{message}");
        }
        assert_eq!(classify("can you push it?"), PUSH_ONLY);
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
            "Do not, under any circumstances, push",
            "Do not commit, or push",
            "Commit and push. Please don't publish it.",
            "The ticket asked for the following:\ncommit and push",
            "Add documentation with this example:\ncommit and push",
            "Add a function to commit and push the branch",
            "The label reads, push",
            "Add a CI script that will lint, test, and push",
            "Write a helper to stage, commit, and push",
            "Update the release docs to cover tag, and push",
            "Commit and push, and merge it",
            "Commit and push, and then push to main.",
            "Commit and push. Then open the PR.",
            "Build the docker image and push it.",
            "Build the container and push it",
            "Run the release script and push it",
            "Build the docker image. Push it.",
            "Commit and push. Then create a new branch called docs-cleanup for the follow-up.",
            "Push the branch. Create a new branch and open a PR for it.",
            "Commit and push the branch.\n- Do not push until I have reviewed the diff.",
            "Commit and push the branch.\n    Do not push until I have reviewed the diff.",
            "Push it.\n- to main",
            "Push it. To another remote.",
            "Push it.\n```text\nto main\n```",
            "```text\nto main\n```\nPush it.",
            "- to main\nPush it.",
            "Commit and push.\n```text\nDo not push until I approve the diff.\n```",
            "```text\nDo not push until I approve the diff.\n```\nCommit and push.",
            "```text\ncommit and push\n```",
            "> commit and push",
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
