//! A fuzzy finder used by Jackdaw
//!
//! The matching is done by the [`FuzzyMatcher`] struct, which stores a list of items
//! which must implement the [`Matchable`] trait.
//!
//! Example:
//!
//! ```
//! use jackdaw_fuzzy::*;
//!
//! /// This item will be matched
//! struct Item {
//!     name: String,
//!     category: String,
//! }
//!
//! impl Matchable for Item {
//!     fn haystack(&self) -> String {
//!         self.name.clone()
//!     }
//!
//!     fn category(&self) -> Category {
//!         Category {
//!             name: Some(self.category.clone()),
//!             order: 0,
//!         }
//!     }
//! }
//!
//! let items = vec![
//!     Item {
//!         name: "Hello there".into(),
//!         category: "Greetings".into(),
//!     },
//!     Item {
//!         name: "Hey!".into(),
//!         category: "Greetings".into(),
//!     },
//!     Item {
//!         name: "How are you?".into(),
//!         category: "Questions".into(),
//!     },
//! ];
//!
//! let mut matcher = FuzzyMatcher::from_items(items);
//! matcher.update_pattern("hey");
//!
//! let matches = matcher.matches();
//!
//! assert_eq!(matches.len(), 2);
//! assert_eq!(matches[0].category.name, Some("Greetings".into()));
//! assert_eq!(matches[0].items[0].index, 1);
//!
//! assert_eq!(matches[1].category.name, Some("Questions".into()));
//! assert_eq!(matches[1].items[0].index, 2);
//!
//! assert!(matches[0].items[0].score > matches[1].items[0].score);
//! ```

use std::collections::HashMap;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// A category that items may be placed in when searching
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone)]
pub struct Category {
    /// The name of the category, if any
    pub name: Option<String>,
    /// The order that the category should appear at. The greater, the earlier it appears
    pub order: i32,
}

/// This trait must be implemented by any item used with a [`FuzzyMatcher`]
pub trait Matchable {
    /// Gets the string that this item should be matched with
    #[must_use]
    fn haystack(&self) -> String;

    /// Gets the category that this item should be placed in
    #[must_use]
    fn category(&self) -> Category {
        Category::default()
    }
}

impl<T: ToString> Matchable for T {
    fn haystack(&self) -> String {
        self.to_string()
    }
}

/// The engine for fuzzy matching.
///
/// It contains a list of items, each of which must implement [`Matchable`], and a pattern which
/// the items are matched against. To set the pattern, use [`update_pattern`](Self::update_pattern) or [`with_pattern`](Self::with_pattern)
///
/// The items may be split into categories if [`Matchable::category`] returns `Some`
#[derive(Debug, Clone)]
pub struct FuzzyMatcher<T: Matchable> {
    items: Vec<T>,
    pattern: Pattern,
    matcher: Matcher,
}

impl<T: Matchable> Default for FuzzyMatcher<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Matchable> FuzzyMatcher<T> {
    /// Creates a new fuzzy matcher with no items and pattern
    #[must_use]
    pub fn new() -> Self {
        Self::from_items(std::iter::empty())
    }

    /// Creates a new fuzzy matcher with items from the given iterator
    #[must_use]
    pub fn from_items(items: impl IntoIterator<Item = T>) -> Self {
        Self {
            items: items.into_iter().collect::<Vec<_>>(),
            pattern: Pattern::parse("", CaseMatching::Smart, Normalization::Smart),
            matcher: Matcher::new(Config::DEFAULT),
        }
    }

    /// Sets the pattern that items are matched against, returning itself
    #[must_use]
    pub fn with_pattern(mut self, pattern: &str) -> Self {
        self.update_pattern(pattern);

        self
    }

    /// Updates the pattern that items are matched against
    pub fn update_pattern(&mut self, pattern: &str) {
        self.pattern
            .reparse(pattern, CaseMatching::Smart, Normalization::Smart);
    }

    /// Adds an item to the item list
    pub fn push_item(&mut self, item: T) {
        self.items.push(item);
    }

    /// Adds an iterator of items to the item list
    pub fn push_items(&mut self, items: impl IntoIterator<Item = T>) {
        self.items.extend(items);
    }

    /// Adds an item to the item list, returning itself
    #[must_use]
    pub fn with_item(mut self, item: T) -> Self {
        self.push_item(item);
        self
    }

    /// Adds an iterator of items to the item list, returning itself
    #[must_use]
    pub fn with_items(mut self, items: impl IntoIterator<Item = T>) -> Self {
        self.push_items(items);
        self
    }

    /// Gets a reference to the list of items
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Compute all the matches and return a slice of categories,
    /// sorted by the highest score in each category in descending order
    #[must_use]
    pub fn matches(&mut self) -> Box<[MatchCategory]> {
        // these buffers are reused in order to not allocate them each computation
        let mut char_buf = vec![]; // used for creating `Utf32Str` when they're non-ascii
        let mut indices = vec![]; // used for getting the match indices

        let mut categories: HashMap<Category, Vec<Match>> =
            HashMap::with_capacity(self.items().len());

        for (index, item) in self.items.iter().enumerate() {
            let haystack = item.haystack();
            let haystack_str = Utf32Str::new(&haystack, &mut char_buf);
            let Some(score) = self.pattern.score(haystack_str, &mut self.matcher) else {
                // if the score is `None`, it doesn't match the pattern at all
                continue;
            };

            // clear the indices, since `Pattern::indices` doesn't
            indices.clear();

            self.pattern
                .indices(haystack_str, &mut self.matcher, &mut indices);

            let mut segments = vec![];
            let mut current_match = MatchedStr {
                text: String::new(),
                is_match: false,
            };

            for (index, char) in haystack_str.chars().enumerate() {
                let is_match = indices.contains(&(index as u32));
                if current_match.is_match != is_match {
                    if !current_match.text.is_empty() {
                        segments.push(current_match);
                    }

                    current_match = MatchedStr {
                        text: String::new(),
                        is_match,
                    };
                }

                current_match.text.push(char);
            }

            if !current_match.text.is_empty() {
                segments.push(current_match);
            }

            let matched = Match {
                segments: segments.into_boxed_slice(),
                haystack,
                score,
                index,
            };

            let category = item.category();
            let entry = categories.entry(category).or_default();
            entry.push(matched);
        }

        let mut categories: Vec<_> = categories
            .into_iter()
            .map(|(category, mut items)| {
                items.sort_by(|a, b| (b.score, &a.haystack).cmp(&(a.score, &b.haystack)));
                MatchCategory {
                    category,
                    items: items.into_boxed_slice(),
                }
            })
            .collect();

        categories.sort_by(|a, b| {
            // the very first item has the highest score in the category
            let score_a = a.items.first().map(|i| i.score).unwrap_or(0);
            let score_b = b.items.first().map(|i| i.score).unwrap_or(0);
            // sort descending in score and order, but ascending in name
            (score_b, b.category.order, &a.category.name).cmp(&(
                score_a,
                a.category.order,
                &b.category.name,
            ))
        });

        categories.into_boxed_slice()
    }
}

/// A category matched by a [`FuzzyMatcher`]
pub struct MatchCategory {
    /// The category info
    pub category: Category,
    /// The items in this category, sorted with the highest scoring items being first
    pub items: Box<[Match]>,
}

/// A single item matched by a [`FuzzyMatcher`]
#[derive(Debug, PartialEq, Clone)]
pub struct Match {
    /// The segments of the matched string, see [`MatchedStr`]
    pub segments: Box<[MatchedStr]>,
    /// How well does the item match the input?
    pub score: u32,
    /// The original haystack (name) of the item
    pub haystack: String,
    /// The index of the underlying item
    pub index: usize,
}

/// An invidiual segment of a [`Match`], which may be used for coloring part of the text
/// if it matches the input string
#[derive(Debug, PartialEq, Clone)]
pub struct MatchedStr {
    /// The part of the string that this segment contains
    pub text: String,
    /// Does this segment match a part of the input string? (Which usually means that it will be highlighted)
    pub is_match: bool,
}
