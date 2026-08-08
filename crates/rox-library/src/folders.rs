//! The folder hierarchy behind the tree panel, reconstructed from the
//! projection's interned folder strings. Building the trie, collapsing the
//! dead prefix above the library, sorting the children, and folding the
//! per-folder counts up the tree are all arithmetic over strings, so they
//! live here and the panel just draws what comes back.

use std::collections::{BTreeMap, HashMap};
use std::path::MAIN_SEPARATOR;

use crate::sort::natural_cmp;

/// One folder in the reconstructed hierarchy. The path is the exact
/// interned folder string, which is what the subtree filter pick matches
/// by prefix; the count is the context tracks in this subtree.
pub struct Node {
    pub label: String,
    pub path: String,
    /// Every song in this subtree, whatever the query - the tree is the
    /// full hierarchy, so this never changes with a search.
    pub total: u32,
    /// Of those, how many pass the active query (text and facet). Equal to
    /// `total` when nothing is active; a subtree with zero here is what a
    /// filter dims or, in Hide mode, drops.
    pub matched: u32,
    pub children: Vec<Node>,
}

/// Reconstruct the folder hierarchy from the projection's folder strings.
/// Every path threads into a trie; the shared prefix above the first
/// branch or the first folder holding tracks collapses away, so the tree
/// starts where the library does instead of at the filesystem root. Node
/// paths slice the original strings, so a pick matches the interned
/// values exactly. Roots and children both sort naturally, case folded;
/// two top nodes that collapse to the same name fall back to their full
/// paths to stay apart.
pub fn build_roots(folders: &[String]) -> Vec<Node> {
    #[derive(Default)]
    struct Trie {
        /// Children keyed by path component, ordered for the walk.
        children: BTreeMap<String, Trie>,
        /// The full path down to this node, sliced from an inserted string.
        path: String,
        /// Whether this exact path is an interned folder - a directory
        /// holding tracks itself, not just the ancestor of one.
        has_tracks: bool,
    }

    // Bare filenames intern to the empty string; drop those.
    let mut root = Trie::default();
    for path in folders.iter().filter(|s| !s.is_empty()) {
        let mut node = &mut root;
        let mut start = 0;
        for (ix, _) in path.match_indices(MAIN_SEPARATOR).chain([(path.len(), "")]) {
            if ix > start {
                let component = path[start..ix].to_string();
                node = node.children.entry(component).or_default();
                if node.path.is_empty() {
                    node.path = path[..ix].to_string();
                }
            }
            start = ix + MAIN_SEPARATOR.len_utf8();
        }
        node.has_tracks = true;
    }

    fn node_from(trie: Trie) -> Node {
        let label = trie
            .path
            .rsplit(MAIN_SEPARATOR)
            .next()
            .unwrap_or(trie.path.as_str())
            .to_string();
        // Natural and case-folded, so "Disc 10" lands after "Disc 2" the way
        // the track rows in the same panel already read. natural_cmp wants
        // lowercase in, and the key is built once per node rather than twice
        // per comparison; the raw label breaks ties so two folders differing
        // only in case keep a stable order.
        let mut keyed: Vec<(String, Node)> = trie
            .children
            .into_values()
            .map(node_from)
            .map(|node| (node.label.to_lowercase(), node))
            .collect();
        keyed.sort_by(|(a_key, a), (b_key, b)| {
            natural_cmp(a_key, b_key).then_with(|| a.label.cmp(&b.label))
        });
        let children: Vec<Node> = keyed.into_iter().map(|(_, node)| node).collect();
        Node {
            label,
            path: trie.path,
            total: 0,
            matched: 0,
            children,
        }
    }

    let tops: Vec<Node> = root
        .children
        .into_values()
        .map(|mut trie| {
            // Collapse the chain of lone, trackless ancestors: /mnt/Zeal
            // holds nothing and branches nowhere, so the top node is Music.
            while !trie.has_tracks && trie.children.len() == 1 {
                trie = trie.children.into_values().next().unwrap();
            }
            node_from(trie)
        })
        .collect();
    let mut keyed: Vec<(String, Node)> = tops
        .into_iter()
        .map(|node| (node.label.to_lowercase(), node))
        .collect();
    keyed.sort_by(|(a_key, a), (b_key, b)| {
        natural_cmp(a_key, b_key).then_with(|| a.label.cmp(&b.label))
    });
    let lower: Vec<String> = keyed.iter().map(|(key, _)| key.clone()).collect();
    let mut tops: Vec<Node> = keyed.into_iter().map(|(_, node)| node).collect();
    for ix in 0..tops.len() {
        let clash = lower
            .iter()
            .enumerate()
            .any(|(other, label)| other != ix && *label == lower[ix]);
        if clash {
            tops[ix].label = tops[ix].path.clone();
        }
    }
    tops
}

/// The node at an exact path, descending only into the branch whose path
/// prefixes the target so the walk stays O(depth), not O(nodes).
pub fn node_at<'a>(nodes: &'a [Node], path: &str) -> Option<&'a Node> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if path.starts_with(node.path.as_str())
            && path[node.path.len()..].starts_with(MAIN_SEPARATOR)
        {
            return node_at(&node.children, path);
        }
    }
    None
}

/// Fold the per-folder counts up the tree: each node's total and matched
/// count are its own folder's plus every descendant's.
pub fn sum_counts(node: &mut Node, by_path: &HashMap<&str, (u32, u32)>) -> (u32, u32) {
    let (mut total, mut matched) = by_path.get(node.path.as_str()).copied().unwrap_or((0, 0));
    for child in &mut node.children {
        let (t, m) = sum_counts(child, by_path);
        total += t;
        matched += m;
    }
    node.total = total;
    node.matched = matched;
    (total, matched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    /// The tree starts where the library does: the lone, trackless chain
    /// above the first real folder collapses into the top node, and the
    /// nesting below reconstructs from the paths alone.
    #[test]
    fn collapses_shared_prefix_and_nests() {
        let roots = build_roots(&folders(&[
            "/mnt/Zeal/Music",
            "/mnt/Zeal/Music/Air - Moon Safari",
            "/mnt/Zeal/Music/Apocalyptica - Cult/CD1",
            "/mnt/Zeal/Music/Apocalyptica - Cult/CD2",
        ]));
        assert_eq!(roots.len(), 1);
        let top = &roots[0];
        assert_eq!(top.label, "Music");
        assert_eq!(top.path, "/mnt/Zeal/Music");
        let labels: Vec<&str> = top.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Air - Moon Safari", "Apocalyptica - Cult"]);
        // The multi-disc album nests its discs; the disc folders carry the
        // exact interned paths so a pick matches them.
        let cult = &top.children[1];
        assert_eq!(cult.children.len(), 2);
        assert_eq!(
            cult.children[0].path,
            "/mnt/Zeal/Music/Apocalyptica - Cult/CD1"
        );
    }

    /// Numbered folders read in disc order, not string order: "Disc 10"
    /// follows "Disc 2" here the same as it does in the track rows below.
    #[test]
    fn children_sort_naturally() {
        let roots = build_roots(&folders(&[
            "/m/Album/Disc 10",
            "/m/Album/Disc 2",
            "/m/Album/Disc 1",
            "/m/Album/disc 3",
        ]));
        let labels: Vec<&str> = roots[0].children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Disc 1", "Disc 2", "disc 3", "Disc 10"]);
    }

    /// Top-level roots follow the same natural order the children do, so
    /// two numbered libraries don't flip at the top of the tree.
    #[test]
    fn roots_sort_naturally() {
        let roots = build_roots(&folders(&[
            "/archive 10/one",
            "/archive 10/two",
            "/archive 2/one",
            "/archive 2/two",
        ]));
        let labels: Vec<&str> = roots.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["archive 2", "archive 10"]);
    }

    /// A folder with tracks stops the collapse even with a single child,
    /// so the top node never skips past real music.
    #[test]
    fn tracks_stop_the_collapse() {
        let roots = build_roots(&folders(["/a/b", "/a/b/c"].as_ref()));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, "/a/b");
        assert_eq!(roots[0].children.len(), 1);
    }

    /// Two libraries that collapse to the same folder name keep their full
    /// paths as labels so the top row stays unambiguous.
    #[test]
    fn clashing_top_labels_fall_back_to_paths() {
        let roots = build_roots(&folders(&[
            "/home/a/Music/X",
            "/home/a/Music/Y",
            "/mnt/media/Music/P",
            "/mnt/media/Music/Q",
            "/srv/Vinyl/Z1",
            "/srv/Vinyl/Z2",
        ]));
        let labels: Vec<&str> = roots.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["/home/a/Music", "/mnt/media/Music", "Vinyl"]);
    }

    /// Counts fold bottom-up: a parent's count is its own tracks plus
    /// every descendant's, folders outside the context at zero. The matched
    /// count folds the same way, so a branch with no facet match reads zero
    /// there while its total stays whole.
    #[test]
    fn counts_aggregate_subtrees() {
        let mut roots = build_roots(&folders(&["/m/Air", "/m/Air/Moon Safari", "/m/Empty"]));
        // (total, matched) per folder: the nested album has songs but none
        // match the active facet, so its matched count is zero.
        let by_path: HashMap<&str, (u32, u32)> =
            [("/m/Air", (2, 2)), ("/m/Air/Moon Safari", (10, 0))]
                .into_iter()
                .collect();
        for root in &mut roots {
            sum_counts(root, &by_path);
        }
        // The collapse stopped at the branch, so the top is /m itself.
        assert_eq!(roots[0].path, "/m");
        assert_eq!(roots[0].total, 12);
        assert_eq!(roots[0].matched, 2);
        let air = &roots[0].children[0];
        assert_eq!(air.total, 12);
        assert_eq!(air.matched, 2);
        assert_eq!(air.children[0].total, 10);
        // The nested album folds no matches, so Dim mode draws it faint.
        assert_eq!(air.children[0].matched, 0);
        assert_eq!(roots[0].children[1].total, 0);
    }
}
