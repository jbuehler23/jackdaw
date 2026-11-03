//! The text that names scene entities wherever an operator takes an entity or
//! a list of them: an id, a name path, or a query over tags and ancestry.
//!
//! ```text
//! 4294967301              an entity id
//! Inn                     every node named Inn, anywhere in the scene
//! village/inn/roof        a path from a top-level node, one name per level
//! village/*/roof          `*` matches any run of characters inside one name
//! #roof                   every node tagged roof
//! #roof&under:village     tagged roof and somewhere below village
//! #window&!#broken        `!` takes a term's matches away
//! ```
//!
//! A text that is exactly the name of one scene node names that node before
//! it is read as a reference at all, so `Wall/Left`, `A&B` or `2` keep naming
//! the node called that. Otherwise `\` makes the next character literal, for
//! a name holding `/`, `&`, `#`, `!`, `*` or `,`, or one made only of digits.
//! Names and tags match exactly, case included. Only scene nodes ever match:
//! never the editor's own entities.

use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;
use jackdaw_scene_types::Tags;
use serde_json::Value;

use crate::EditorEntity;

/// Why a reference named nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    /// The text is not a reference; the reason says why.
    Malformed(String),
    /// A well-formed reference that no node in the scene answers to.
    NoMatch,
}

impl RefError {
    /// The refusal a caller reads, naming the text it sent.
    pub fn describe(&self, text: &str) -> String {
        match self {
            Self::Malformed(why) => format!("`{text}` is not an entity reference: {why}"),
            Self::NoMatch => format!("`{text}` matches no entity in this scene"),
        }
    }
}

/// A parsed reference: terms joined by `&`, each optionally negated.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    terms: Vec<Term>,
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    negated: bool,
    atom: Atom,
}

#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Id(u64),
    Tag(String),
    Under(Box<Atom>),
    Path(Vec<Pattern>),
}

/// One name in a path, with `*` wildcards.
#[derive(Debug, Clone, PartialEq)]
struct Pattern(Vec<Piece>);

#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Text(String),
    Star,
}

/// A character of reference text, and whether a `\` made it literal.
type Char = (char, bool);

fn malformed(why: impl Into<String>) -> RefError {
    RefError::Malformed(why.into())
}

impl Reference {
    pub fn parse(text: &str) -> Result<Self, RefError> {
        let chars = unescape(text)?;
        let mut terms = Vec::new();
        for term in split_on(&chars, '&') {
            let term = trim(term);
            if term.is_empty() {
                return Err(malformed("it has an empty term"));
            }
            let (negated, rest) = match term.split_first() {
                Some((('!', false), rest)) => (true, trim(rest)),
                _ => (false, term),
            };
            terms.push(Term {
                negated,
                atom: parse_atom(rest)?,
            });
        }
        if terms.iter().all(|term| term.negated) {
            return Err(malformed(
                "every term is negated, so there is nothing to take matches from",
            ));
        }
        Ok(Self { terms })
    }

    /// The scene nodes this reference matches, in outliner order.
    pub fn matches(&self, world: &mut World) -> Vec<Entity> {
        let found = self.matched_set(world);
        in_outliner_order(world, found)
    }

    fn matched_set(&self, world: &mut World) -> EntityHashSet {
        let mut kept: Option<EntityHashSet> = None;
        for term in self.terms.iter().filter(|term| !term.negated) {
            let found = eval(world, &term.atom);
            kept = Some(match kept {
                None => found,
                Some(kept) => kept.intersection(&found).copied().collect(),
            });
        }
        let mut kept = kept.unwrap_or_default();
        for term in self.terms.iter().filter(|term| term.negated) {
            for entity in eval(world, &term.atom) {
                kept.remove(&entity);
            }
        }
        kept.retain(|&entity| is_scene_member(world, entity));
        kept
    }
}

/// Every scene node `text` matches, or why it matches none.
pub fn resolve(world: &mut World, text: &str) -> Result<Vec<Entity>, RefError> {
    let found = resolve_set(world, text)?;
    Ok(in_outliner_order(world, found))
}

fn resolve_set(world: &mut World, text: &str) -> Result<EntityHashSet, RefError> {
    if let Some(entity) = exactly_named(world, text) {
        return Ok([entity].into_iter().collect());
    }
    let found = Reference::parse(text)?.matched_set(world);
    if found.is_empty() {
        return Err(RefError::NoMatch);
    }
    Ok(found)
}

/// The one scene node whose `Name` is `text` exactly, if exactly one is.
pub fn exactly_named(world: &mut World, text: &str) -> Option<Entity> {
    let mut named = world.query_filtered::<(Entity, &Name), Without<EditorEntity>>();
    let candidates: Vec<Entity> = named
        .iter(world)
        .filter(|(_, name)| name.as_str() == text)
        .map(|(entity, _)| entity)
        .collect();
    let mut members = candidates
        .into_iter()
        .filter(|&entity| is_scene_member(world, entity));
    match (members.next(), members.next()) {
        (Some(entity), None) => Some(entity),
        _ => None,
    }
}

/// Whether `entity` is a node of the open scene: one the outliner shows, or
/// one the scene document holds, and never the editor's own furniture.
pub fn is_scene_member(world: &World, entity: Entity) -> bool {
    let Ok(found) = world.get_entity(entity) else {
        return false;
    };
    if found.contains::<EditorEntity>() || found.contains::<jackdaw_scene_types::EditorHidden>() {
        return false;
    }
    crate::remote::server::belongs_to_the_scene(world, entity)
        || world
            .get_resource::<jackdaw_bsn::SceneBsnAst>()
            .is_some_and(|doc| doc.ast_for(entity).is_some())
}

/// The one scene node `text` matches. A reference matching several is refused
/// with the count, since picking one would be a guess.
pub fn resolve_one(world: &mut World, text: &str) -> Result<Entity, String> {
    match resolve(world, text) {
        Ok(found) if found.len() == 1 => Ok(found[0]),
        Ok(found) => Err(format!(
            "`{text}` matches {} entities; this takes one",
            found.len()
        )),
        Err(err) => Err(err.describe(text)),
    }
}

/// The nodes a list of references matches, without repeats, in outliner
/// order. Each item has to match something.
pub fn resolve_list(world: &mut World, text: &str) -> Result<Vec<Entity>, String> {
    let mut found = EntityHashSet::default();
    for item in list_items(text)? {
        found.extend(resolve_set(world, &item).map_err(|err| err.describe(&item))?);
    }
    Ok(in_outliner_order(world, found))
}

/// The references in a list: a JSON array (nested arrays flatten, numbers are
/// ids), or text split on unescaped commas. Space-separated ids still read as
/// several ids.
pub fn list_items(text: &str) -> Result<Vec<String>, String> {
    let trimmed = text.trim();
    if trimmed.starts_with('[')
        && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed)
    {
        let mut out = Vec::new();
        flatten(&items, &mut out)?;
        return Ok(out);
    }
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed);
    let mut out = Vec::new();
    for item in split_list(inner) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let ids: Vec<&str> = item.split_whitespace().collect();
        if ids.len() > 1 && ids.iter().all(|id| id.bytes().all(|b| b.is_ascii_digit())) {
            out.extend(ids.into_iter().map(str::to_string));
        } else {
            out.push(item.to_string());
        }
    }
    Ok(out)
}

fn flatten(items: &[Value], out: &mut Vec<String>) -> Result<(), String> {
    for item in items {
        match item {
            Value::String(text) => out.push(text.clone()),
            Value::Number(number) if number.is_u64() => out.push(number.to_string()),
            Value::Array(inner) => flatten(inner, out)?,
            other => return Err(format!("{other} is not an entity reference")),
        }
    }
    Ok(())
}

/// `text` split on commas a `\` does not escape, escapes kept for the parser.
fn split_list(text: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == ',' {
            items.push(&text[start..at]);
            start = at + 1;
        }
    }
    items.push(&text[start..]);
    items
}

/// Whether `text` reads as a reference rather than a plain name: it uses a
/// tag, a negation, `under:`, a path or `&`.
pub fn looks_like_query(text: &str) -> bool {
    let text = text.trim();
    text.starts_with('#')
        || text.starts_with('!')
        || text.starts_with("under:")
        || text.contains('/')
        || text.contains('&')
}

fn unescape(text: &str) -> Result<Vec<Char>, RefError> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let next = chars
                .next()
                .ok_or_else(|| malformed("it ends in a lone `\\`"))?;
            out.push((next, true));
        } else {
            out.push((c, false));
        }
    }
    Ok(out)
}

fn split_on(chars: &[Char], separator: char) -> Vec<&[Char]> {
    chars
        .split(|&(c, escaped)| c == separator && !escaped)
        .collect()
}

fn trim(chars: &[Char]) -> &[Char] {
    let kept = |&(c, escaped): &Char| escaped || !c.is_whitespace();
    let Some(start) = chars.iter().position(kept) else {
        return &[];
    };
    let end = chars.iter().rposition(kept).map_or(start, |at| at + 1);
    &chars[start..end]
}

fn plain(chars: &[Char]) -> String {
    chars.iter().map(|(c, _)| *c).collect()
}

fn strip_word<'a>(chars: &'a [Char], word: &str) -> Option<&'a [Char]> {
    let len = word.chars().count();
    (chars.len() >= len
        && chars
            .iter()
            .zip(word.chars())
            .all(|(&(c, escaped), wanted)| !escaped && c == wanted))
    .then(|| &chars[len..])
}

fn parse_atom(chars: &[Char]) -> Result<Atom, RefError> {
    if chars.is_empty() {
        return Err(malformed("it has an empty term"));
    }
    if let Some((('#', false), tag)) = chars.split_first() {
        let tag = plain(tag);
        if let Some(why) = Tags::refuse(&tag) {
            return Err(RefError::Malformed(why));
        }
        return Ok(Atom::Tag(tag));
    }
    if let Some(rest) = strip_word(chars, "under:") {
        return Ok(Atom::Under(Box::new(parse_atom(trim(rest))?)));
    }
    if chars
        .iter()
        .all(|&(c, escaped)| !escaped && c.is_ascii_digit())
    {
        return plain(chars)
            .parse::<u64>()
            .map(Atom::Id)
            .map_err(|_| malformed("the number is too large to be an entity id"));
    }
    let mut path = Vec::new();
    for segment in split_on(chars, '/') {
        if segment.is_empty() {
            return Err(malformed(
                "a path has an empty name; write `a/b`, not `a//b`, `/a` or `a/`",
            ));
        }
        path.push(pattern(segment)?);
    }
    Ok(Atom::Path(path))
}

fn pattern(chars: &[Char]) -> Result<Pattern, RefError> {
    let mut pieces = Vec::new();
    for &(c, escaped) in chars {
        if c == '*' && !escaped {
            if pieces.last() == Some(&Piece::Star) {
                return Err(malformed(
                    "`**` is not offered; `*` matches inside one name",
                ));
            }
            pieces.push(Piece::Star);
            continue;
        }
        match pieces.last_mut() {
            Some(Piece::Text(text)) => text.push(c),
            _ => pieces.push(Piece::Text(c.to_string())),
        }
    }
    Ok(Pattern(pieces))
}

impl Pattern {
    fn matches(&self, name: &str) -> bool {
        let pattern: Vec<Option<char>> = self
            .0
            .iter()
            .flat_map(|piece| match piece {
                Piece::Star => vec![None],
                Piece::Text(text) => text.chars().map(Some).collect(),
            })
            .collect();
        let name: Vec<char> = name.chars().collect();
        glob(&pattern, &name)
    }
}

/// Wildcard match in linear passes: `None` is a `*`. On a mismatch the last
/// star takes one more character and the match resumes after it.
fn glob(pattern: &[Option<char>], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(None) => {
                star = Some((p, t));
                p += 1;
            }
            Some(Some(c)) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(Option::is_none)
}

/// What a path segment matches a node by: its name, or for an unnamed prefab
/// instance its source's file stem, as the outliner labels it.
fn label(world: &World, entity: Entity) -> Option<String> {
    world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .or_else(|| crate::hierarchy::prefab_stem_label(world, entity))
}

fn eval(world: &mut World, atom: &Atom) -> EntityHashSet {
    match atom {
        Atom::Id(bits) => Entity::try_from_bits(*bits)
            .filter(|entity| is_scene_member(world, *entity))
            .into_iter()
            .collect(),
        Atom::Tag(tag) => {
            let mut tagged = world.query_filtered::<(Entity, &Tags), Without<EditorEntity>>();
            tagged
                .iter(world)
                .filter(|(_, tags)| tags.contains(tag))
                .map(|(entity, _)| entity)
                .collect()
        }
        Atom::Under(inner) => {
            let mut below = EntityHashSet::default();
            let mut stack: Vec<Entity> = eval(world, inner).into_iter().collect();
            while let Some(entity) = stack.pop() {
                let children: Vec<Entity> = world
                    .get::<Children>(entity)
                    .map(|children| children.to_vec())
                    .unwrap_or_default();
                for child in children {
                    if is_scene_member(world, child) && below.insert(child) {
                        stack.push(child);
                    }
                }
            }
            below
        }
        Atom::Path(path) => match path.as_slice() {
            [only] => named_anywhere(world, only),
            [first, rest @ ..] => {
                let mut level: Vec<Entity> = crate::remote::server::scene_roots(world)
                    .into_iter()
                    .filter(|&root| label(world, root).is_some_and(|name| first.matches(&name)))
                    .collect();
                for step in rest {
                    level = level
                        .into_iter()
                        .flat_map(|parent| {
                            world
                                .get::<Children>(parent)
                                .map(|children| children.iter().collect::<Vec<_>>())
                                .unwrap_or_default()
                        })
                        .filter(|&child| {
                            crate::remote::server::is_scene_descendant(world, child)
                                && label(world, child).is_some_and(|name| step.matches(&name))
                        })
                        .collect();
                }
                level.into_iter().collect()
            }
            [] => EntityHashSet::default(),
        },
    }
}

/// Every authored node whose name matches, wherever it sits; with none, every
/// unnamed prefab instance whose source's stem does.
fn named_anywhere(world: &mut World, pattern: &Pattern) -> EntityHashSet {
    let mut named = world.query_filtered::<(Entity, &Name), Without<EditorEntity>>();
    let found: EntityHashSet = named
        .iter(world)
        .filter(|(_, name)| pattern.matches(name.as_str()))
        .map(|(entity, _)| entity)
        .collect();
    if !found.is_empty() {
        return found;
    }
    let mut instances = world
        .query_filtered::<(Entity, &crate::prefab::IsA), (Without<Name>, Without<EditorEntity>)>();
    instances
        .iter(world)
        .filter(|(_, isa)| {
            isa.source
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| pattern.matches(stem))
        })
        .map(|(entity, _)| entity)
        .collect()
}

/// `found` sorted by each node's place in the outliner, computed once.
fn in_outliner_order(world: &mut World, found: EntityHashSet) -> Vec<Entity> {
    let order = outliner_order(world);
    let mut found: Vec<Entity> = found.into_iter().collect();
    found.sort_by_key(|entity| (order.get(entity).copied().unwrap_or(usize::MAX), *entity));
    found
}

/// Each scene node's place in the outliner: top-level nodes by id, then each
/// node's children in their order, depth first.
fn outliner_order(world: &mut World) -> EntityHashMap<usize> {
    let mut roots = crate::remote::server::scene_roots(world);
    roots.sort();
    let mut order = EntityHashMap::default();
    let mut stack: Vec<Entity> = roots.into_iter().rev().collect();
    while let Some(entity) = stack.pop() {
        if order.contains_key(&entity) {
            continue;
        }
        let place = order.len();
        order.insert(entity, place);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter().rev());
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Reference {
        Reference::parse(text).unwrap_or_else(|err| panic!("{}", err.describe(text)))
    }

    fn refused(text: &str) -> String {
        match Reference::parse(text) {
            Ok(parsed) => panic!("`{text}` parsed as {parsed:?}"),
            Err(err) => err.describe(text),
        }
    }

    #[test]
    fn digits_are_an_id_and_an_escaped_digit_is_a_name() {
        assert_eq!(parse("42").terms[0].atom, Atom::Id(42));
        assert_eq!(
            parse("\\42").terms[0].atom,
            Atom::Path(vec![Pattern(vec![Piece::Text("42".into())])])
        );
    }

    #[test]
    fn a_path_splits_on_unescaped_slashes_only() {
        let Atom::Path(path) = parse("village/a\\/b/*roof").terms[0].atom.clone() else {
            panic!("a path");
        };
        assert_eq!(path.len(), 3);
        assert!(path[1].matches("a/b"));
        assert!(path[2].matches("roof") && path[2].matches("redroof"));
        assert!(!path[2].matches("roofs"));
    }

    #[test]
    fn terms_join_on_ampersands_with_or_without_spaces() {
        assert_eq!(parse("#roof & under:village").terms.len(), 2);
        let terms = parse("#roof&!#broken").terms;
        assert_eq!(terms.len(), 2);
        assert!(terms[1].negated);
        assert_eq!(terms[1].atom, Atom::Tag("broken".into()));
    }

    #[test]
    fn malformed_references_say_why() {
        for (text, why) in [
            ("a//b", "empty name"),
            ("/a", "empty name"),
            ("a/", "empty name"),
            ("#", "not empty"),
            ("#a b", "holds ` `"),
            ("a**", "`**`"),
            ("!#roof", "every term is negated"),
            ("#roof&", "empty term"),
            ("a\\", "lone"),
        ] {
            let line = refused(text);
            assert!(line.contains(why), "{text}: {line}");
        }
    }

    #[test]
    fn a_star_matches_any_run_inside_one_name() {
        let star = pattern(&unescape("ro*f").unwrap()).unwrap();
        assert!(star.matches("roof") && star.matches("rof") && star.matches("ro/of"));
        assert!(!star.matches("Roof"), "names match with their case");
        let literal = pattern(&unescape("a\\*").unwrap()).unwrap();
        assert!(literal.matches("a*") && !literal.matches("ab"));
    }

    #[test]
    fn a_pathological_pattern_answers_without_backtracking_blowup() {
        let star = pattern(&unescape(&format!("{}b", "a*".repeat(30))).unwrap()).unwrap();
        let started = std::time::Instant::now();
        assert!(!star.matches(&"a".repeat(200)));
        assert!(star.matches(&format!("{}b", "a".repeat(200))));
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
    }

    #[test]
    fn a_list_reads_json_commas_and_space_separated_ids() {
        assert_eq!(
            list_items(r##"[1, "#roof & under:inn", [2]]"##).unwrap(),
            vec!["1", "#roof & under:inn", "2"]
        );
        assert_eq!(
            list_items("1,2 3, #roof").unwrap(),
            vec!["1", "2", "3", "#roof"]
        );
        assert_eq!(list_items("[1,2]").unwrap(), vec!["1", "2"]);
        assert_eq!(list_items("a\\,b,c").unwrap(), vec!["a\\,b", "c"]);
        assert!(list_items("[true]").is_err());
    }

    #[test]
    fn only_query_shaped_text_leaves_the_plain_search() {
        for query in ["#roof", "!#a&#b", "under:inn", "village/inn", "a&b"] {
            assert!(looks_like_query(query), "{query}");
        }
        for plain in ["roof", "Big Tree", "Cube2"] {
            assert!(!looks_like_query(plain), "{plain}");
        }
    }
}
