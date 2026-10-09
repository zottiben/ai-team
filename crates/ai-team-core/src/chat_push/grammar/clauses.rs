//! Parse complete instructions, preserving sentence context. In particular, a comma
//! does not turn a described operation ("a script that will test, and push") into consent.

use super::{parse, Intent, CONJ, FILLER, SUBJECT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewBranch {
    pub base: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub intent: Intent,
    pub branch: Option<String>,
    pub new_branch: Option<NewBranch>,
    pub work: bool,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            intent: Intent::None,
            branch: None,
            new_branch: None,
            work: false,
        }
    }
}

pub(crate) fn read(message: &str) -> Request {
    let mut result = Request::default();
    if message.len() > 32_000 {
        return result;
    }
    let text = message.replace('\u{2019}', "'");
    let sentences = sentences(&text);
    let tokens: Vec<Vec<String>> = sentences
        .iter()
        .map(|s| without_pr_constraint(s.split_whitespace().map(str::to_owned).collect()))
        .collect();
    if sentences.iter().any(|s| s.trim_end().ends_with(':'))
        || tokens
            .iter()
            .any(|words| reported(words) || negated_push(words))
    {
        result.intent = super::mention(&text);
        return result;
    }
    let other_publication = tokens.iter().any(|words| non_git_work(words));
    let conditional = tokens.iter().any(|words| {
        !aside(words)
            && words.iter().any(|w| {
                matches!(
                    w.to_ascii_lowercase().as_str(),
                    "if" | "unless" | "when" | "once" | "after" | "until"
                )
            })
    });
    let mut approvals = 0;
    let mut unresolved_push = false;
    for words in &tokens {
        if words.is_empty() {
            continue;
        }
        if let Some(instruction) = instruction(words) {
            approvals += 1;
            if approvals > 1
                || conditional
                || (other_publication && !words.iter().any(|w| w.eq_ignore_ascii_case("branch")))
            {
                result.intent = Intent::Ambiguous;
                return result;
            }
            result.intent = instruction.intent;
            result.branch = instruction.branch;
            result.work |= instruction.work;
            if instruction.new_branch.is_some() {
                if result.new_branch.is_some() {
                    result.intent = Intent::Ambiguous;
                    return result;
                }
                result.new_branch = instruction.new_branch;
            }
        } else if let Some(new) = creation(words) {
            if result.new_branch.is_some() || approvals > 0 {
                result.intent = Intent::Ambiguous;
                return result;
            }
            result.new_branch = Some(new);
            result.work = true;
        } else if !aside(words) && !acknowledgement(words) {
            result.work = true;
            unresolved_push |= approvals > 0 || apparent_delivery(words);
        }
    }
    if approvals == 0 {
        result.intent = super::mention(&text);
    } else if unresolved_push {
        result.intent = Intent::Ambiguous;
    }
    if let (Some(name), Some(new)) = (&result.branch, &mut result.new_branch) {
        if new.name.as_ref().is_some_and(|wanted| wanted != name) {
            result.intent = Intent::Ambiguous;
        } else {
            new.name = Some(name.clone());
        }
    }
    result
}

fn without_pr_constraint(words: Vec<String>) -> Vec<String> {
    let lower: Vec<_> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let end = lower.len() - usize::from(lower.last().is_some_and(|w| w == "please"));
    for suffix in [
        &["no", "pr"][..],
        &["no", "prs"],
        &["don't", "open", "a", "pr"],
        &["do", "not", "open", "a", "pr"],
        &["don't", "create", "a", "pr"],
    ] {
        if end >= suffix.len()
            && lower[end - suffix.len()..end]
                .iter()
                .map(String::as_str)
                .eq(suffix.iter().copied())
        {
            let mut prefix = words[..end - suffix.len()].to_vec();
            if prefix
                .last()
                .is_some_and(|w| matches!(w.to_ascii_lowercase().as_str(), "and" | "but"))
            {
                prefix.pop();
            }
            return prefix;
        }
    }
    words
}

fn non_git_work(words: &[String]) -> bool {
    let head = words.first().map(|w| w.to_ascii_lowercase());
    match head.as_deref() {
        Some("build" | "create" | "make") => words.iter().any(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "image" | "container" | "docker" | "package" | "registry" | "artifact" | "bundle"
            )
        }),
        Some("run") => words
            .iter()
            .any(|w| matches!(w.to_ascii_lowercase().as_str(), "release" | "script")),
        _ => false,
    }
}

fn instruction(words: &[String]) -> Option<Request> {
    if let Some((commit, branch)) = directive(words) {
        return Some(Request {
            intent: Intent::Authorize { commit },
            branch,
            ..Request::default()
        });
    }
    // A direct task can end in an instruction to publish its result. Never extract a
    // verb from what a script, function, label or document is being asked to describe.
    for at in 1..words.len().saturating_sub(1) {
        if !matches!(words[at].to_ascii_lowercase().as_str(), "and" | "then") {
            continue;
        }
        let prefix = &words[..at];
        let new_branch = creation(prefix);
        if new_branch.is_none() && !direct_task(prefix) {
            continue;
        }
        if let Some((commit, branch)) = directive(&words[at + 1..]) {
            return Some(Request {
                intent: Intent::Authorize { commit },
                branch,
                new_branch,
                work: true,
            });
        }
    }
    None
}

fn direct_task(words: &[String]) -> bool {
    words.first().is_some_and(|w| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "fix"
                | "upgrade"
                | "update"
                | "refactor"
                | "implement"
                | "build"
                | "run"
                | "test"
                | "add"
        )
    }) && !words.iter().any(|w| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "to" | "that"
                | "which"
                | "will"
                | "would"
                | "can"
                | "should"
                | "not"
                | "don't"
                | "dont"
                | "never"
                | "quoted-content"
                | "push"
                | "commit"
        )
    })
}

fn reported(words: &[String]) -> bool {
    let lower: Vec<_> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    [
        &["they", "said"][..],
        &["they", "asked"],
        &["for", "example"],
        &["the", "reviewer"],
        &["the", "ticket", "says"],
    ]
    .iter()
    .any(|prefix| {
        lower
            .iter()
            .take(prefix.len())
            .map(String::as_str)
            .eq(prefix.iter().copied())
    })
}

fn negated_push(words: &[String]) -> bool {
    let lower: Vec<_> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    let negative = lower.iter().any(|w| {
        matches!(
            w.as_str(),
            "not" | "never" | "don't" | "dont" | "without" | "avoid" | "no"
        )
    });
    negative
        && lower.iter().any(|w| {
            matches!(
                w.as_str(),
                "push" | "pushed" | "pushing" | "publish" | "published" | "publishing"
            )
        })
}

fn aside(words: &[String]) -> bool {
    words
        .first()
        .is_some_and(|w| matches!(w.to_ascii_lowercase().as_str(), "i'll" | "ill" | "we'll"))
}

fn acknowledgement(words: &[String]) -> bool {
    words.iter().all(|w| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "the"
                | "tests"
                | "checks"
                | "passed"
                | "pass"
                | "looks"
                | "this"
                | "that"
                | "ready"
                | "great"
                | "yes"
                | "sure"
                | "look"
                | "good"
                | "thanks"
                | "thank"
                | "you"
                | "ok"
                | "okay"
                | "please"
        )
    }) || words
        .iter()
        .any(|w| matches!(w.to_ascii_lowercase().as_str(), "not" | "no" | "don't"))
        && words
            .iter()
            .any(|w| matches!(w.to_ascii_lowercase().as_str(), "pr" | "prs"))
        && !words.iter().any(|w| {
            matches!(
                w.to_ascii_lowercase().as_str(),
                "push" | "publish" | "commit"
            )
        })
}

fn apparent_delivery(words: &[String]) -> bool {
    // Quoted delivery context can make approval ambiguous, never authorise it.
    let words = if words.first().is_some_and(|word| word == "quoted-content") {
        &words[1..]
    } else {
        words
    };
    let mut original = words.to_vec();
    let mut lower: Vec<_> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    strip_prefix(&mut lower, &mut original);
    lower.first().is_some_and(|w| {
        matches!(
            w.as_str(),
            "push" | "publish" | "commit" | "merge" | "tag" | "force" | "delete" | "to" | "onto"
        )
    }) || lower.starts_with(&["use".into(), "remote".into()])
        || lower
            .first()
            .is_some_and(|w| matches!(w.as_str(), "open" | "create"))
            && lower
                .iter()
                .any(|w| matches!(w.as_str(), "pr" | "pull" | "release"))
}

fn strip_prefix(words: &mut Vec<String>, original: &mut Vec<String>) {
    while words.first().is_some_and(|w| {
        matches!(
            w.as_str(),
            "please"
                | "ok"
                | "okay"
                | "yes"
                | "sure"
                | "great"
                | "thanks"
                | "then"
                | "and"
                | "also"
        )
    }) {
        words.remove(0);
        original.remove(0);
    }
    for prefix in [
        &["looks", "good"][..],
        &["this", "looks", "good"],
        &["all", "good"],
        &["i", "want", "you", "to"],
        &["i", "would", "like", "you", "to"],
        &["i'd", "like", "you", "to"],
        &["i", "would", "like"],
        &["i", "want"],
        &["i'd", "like"],
        &["go", "ahead", "and"],
        &["can", "you"],
        &["could", "you"],
        &["would", "you"],
    ] {
        if words
            .iter()
            .take(prefix.len())
            .map(String::as_str)
            .eq(prefix.iter().copied())
        {
            words.drain(..prefix.len());
            original.drain(..prefix.len());
            break;
        }
    }
    while words
        .first()
        .is_some_and(|w| matches!(w.as_str(), "please" | "and" | "then"))
    {
        words.remove(0);
        original.remove(0);
    }
}

fn directive(original: &[String]) -> Option<(bool, Option<String>)> {
    if original.len() > 48 {
        return None;
    }
    let mut original = original.to_vec();
    let mut words: Vec<_> = original.iter().map(|w| w.to_ascii_lowercase()).collect();
    let wanted = words.starts_with(&["i".into(), "want".into()])
        || words.starts_with(&["i'd".into(), "like".into()])
        || words.starts_with(&["i".into(), "would".into(), "like".into()]);
    strip_prefix(&mut words, &mut original);
    // Preserve the common "The commit and push" keyboard typo for "Then".
    if words.first().is_some_and(|w| w == "the") && words.get(1).is_some_and(|w| w == "commit") {
        words.remove(0);
        original.remove(0);
    }
    if words.last().is_some_and(|w| w == "please") {
        words.pop();
        original.pop();
    }
    if wanted && words.last().is_some_and(|w| w == "pushed") {
        words.pop();
        original.pop();
        words.insert(0, "push".into());
        original.insert(0, "push".into());
    }
    if !words
        .first()
        .is_some_and(|w| matches!(w.as_str(), "push" | "commit"))
    {
        return None;
    }
    let mut branch = None;
    if let Some(at) = words.windows(2).position(|pair| {
        pair[0] == "branch"
            && !FILLER.contains(&pair[1].as_str())
            && !CONJ.contains(&pair[1].as_str())
            && !SUBJECT.contains(&pair[1].as_str())
    }) {
        let named = matches!(words[at + 1].as_str(), "named" | "called");
        let index = at + if named { 2 } else { 1 };
        let candidate = original.get(index)?;
        if !reference(candidate) || (!named && candidate.eq_ignore_ascii_case("upstream")) {
            return None;
        }
        branch = Some(candidate.clone());
        words.remove(index);
        if named {
            words.remove(at + 1);
        }
    }
    match parse(&words) {
        Some((true, commit)) => Some((commit, branch)),
        _ => None,
    }
}

fn creation(original: &[String]) -> Option<NewBranch> {
    let mut original = original.to_vec();
    let mut words: Vec<_> = original.iter().map(|w| w.to_ascii_lowercase()).collect();
    strip_prefix(&mut words, &mut original);
    if !words
        .first()
        .is_some_and(|w| matches!(w.as_str(), "create" | "make" | "start"))
    {
        return None;
    }
    let mut at = 1;
    if words
        .get(at)
        .is_some_and(|w| matches!(w.as_str(), "a" | "and"))
    {
        at += 1;
    }
    if words.get(at).is_none_or(|w| w != "new") || words.get(at + 1).is_none_or(|w| w != "branch") {
        return None;
    }
    if words.iter().any(|w| {
        matches!(
            w.as_str(),
            "push" | "commit" | "not" | "don't" | "dont" | "never" | "if"
        )
    }) {
        return None;
    }
    let base = if let Some(at) = words
        .iter()
        .position(|w| matches!(w.as_str(), "from" | "off"))
    {
        let at = (at + 1..words.len())
            .find(|&i| !matches!(words[i].as_str(), "of" | "the" | "latest"))?;
        let value = original.get(at)?;
        if !reference(value) {
            return None;
        }
        Some(value.clone())
    } else {
        None
    };
    let name = if let Some(at) = words
        .iter()
        .position(|w| matches!(w.as_str(), "named" | "called"))
    {
        let value = original.get(at + 1)?;
        if !reference(value) {
            return None;
        }
        Some(value.clone())
    } else {
        None
    };
    Some(NewBranch { base, name })
}

fn reference(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && !word.contains("..")
        && word
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_./".contains(&c))
}

/// Quotes/code are opaque, not deleted into a valid instruction. Sentence boundaries
/// permit separate direct approval; commas remain part of the same instruction.
fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut fenced = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(format!("quoted-content {trimmed}"));
            continue;
        }
        if trimmed.starts_with(['>', '-', '*']) || line.starts_with("    ") {
            // Formatting cannot erase a prohibition or turn a qualifier into consent.
            // This marker prevents a list/quote from becoming a direct instruction.
            out.push(format!(
                "quoted-content {}",
                trimmed.trim_start_matches(['>', '-', '*', ' '])
            ));
            continue;
        }
        let chars: Vec<_> = line.chars().collect();
        for (at, &c) in chars.iter().enumerate() {
            if let Some(end) = quote {
                if c == end {
                    quote = None;
                }
                continue;
            }
            if matches!(c, '"' | '`' | '“' | '”')
                || (c == '\'' && (at == 0 || !chars[at - 1].is_alphanumeric()))
            {
                current.push_str(" quoted-content ");
                quote = Some(if c == '“' { '”' } else { c });
            } else if matches!(c, ';' | '?' | '!')
                || (c == '.' && chars.get(at + 1).is_none_or(|c| c.is_whitespace()))
            {
                out.push(std::mem::take(&mut current));
            } else {
                current.push(if c == ',' { ' ' } else { c });
            }
        }
        out.push(std::mem::take(&mut current));
    }
    out
}
