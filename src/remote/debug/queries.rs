//! Live-query view model. `QuerySpec` is the pure filter the panel edits and
//! turns into BRP `world.query` parameters; the panel UI is added in a later
//! task.

use serde_json::json;

/// A `With`/`Without` component filter over the running world's entities.
/// Component names are full reflect type paths (for example
/// `skybound::Enemy`).
#[derive(Debug, Clone, Default)]
pub struct QuerySpec {
    pub with: Vec<String>,
    pub without: Vec<String>,
}

impl QuerySpec {
    /// True when a component set satisfies the filter: it holds every `with`
    /// component and none of the `without` ones.
    pub fn matches(&self, comps: &[String]) -> bool {
        self.with.iter().all(|c| comps.contains(c))
            && self.without.iter().all(|c| !comps.contains(c))
    }

    /// The parameters for a BRP `world.query` request built from this filter.
    pub fn to_brp_params(&self) -> serde_json::Value {
        json!({
            "data": { "components": self.with, "has": [], "option": [] },
            "filter": { "with": self.with, "without": self.without }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::QuerySpec;

    #[test]
    fn matches_requires_all_with_and_no_without() {
        let q = QuerySpec {
            with: vec!["Enemy".into(), "Health".into()],
            without: vec!["Dead".into()],
        };
        assert!(q.matches(&["Enemy".into(), "Health".into(), "Collider".into()]));
        assert!(!q.matches(&["Enemy".into()]));
        assert!(!q.matches(&["Enemy".into(), "Health".into(), "Dead".into()]));
    }

    #[test]
    fn brp_params_lists_components_by_full_path() {
        let q = QuerySpec {
            with: vec!["skybound::Enemy".into()],
            without: vec![],
        };
        let p = q.to_brp_params();
        assert_eq!(p["data"]["components"][0], "skybound::Enemy");
    }
}
