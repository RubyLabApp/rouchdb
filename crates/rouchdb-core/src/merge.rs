/// Revision tree merge algorithm.
///
/// Implements the same logic as PouchDB's `pouchdb-merge` module:
/// - Merge incoming revision paths into an existing tree
/// - Determine the winning revision deterministically
/// - Stem (prune) old revisions beyond a configurable limit
use crate::document::Revision;
use crate::rev_tree::{RevNode, RevPath, RevStatus, RevTree, collect_leaves};

/// Result of merging a new path into the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    /// The path extended an existing branch (normal edit).
    NewLeaf,
    /// The path created a new branch (conflict).
    NewBranch,
    /// The path's leaf already existed in the tree (duplicate/no-op).
    InternalNode,
}

/// Merge a new revision path into the existing tree.
///
/// Returns the updated tree and a `MergeResult` indicating what happened.
pub fn merge_tree(tree: &RevTree, new_path: &RevPath, rev_limit: u64) -> (RevTree, MergeResult) {
    let mut result_tree = tree.clone();
    let merge_result = do_merge(&mut result_tree, new_path);

    // Apply stemming if we have a rev_limit
    if rev_limit > 0 {
        let _stemmed = stem(&mut result_tree, rev_limit);
    }

    (result_tree, merge_result)
}

/// Core merge logic. Tries to merge `new_path` into `tree`, modifying it
/// in place.
fn do_merge(tree: &mut RevTree, new_path: &RevPath) -> MergeResult {
    // Try to merge the new path into each existing root
    for existing in tree.iter_mut() {
        let result = try_merge_path(existing, new_path);
        if let Some(merge_result) = result {
            return merge_result;
        }
    }

    // No overlap found — this is a completely new branch
    tree.push(new_path.clone());
    MergeResult::NewBranch
}

/// Try to merge `new_path` into a single existing `RevPath`.
///
/// Returns `None` if the paths don't share any common ancestor (no overlap),
/// meaning the new path should be tried against other roots or added as a
/// new root.
fn try_merge_path(existing: &mut RevPath, new_path: &RevPath) -> Option<MergeResult> {
    // Find the overlap point between the two paths
    let overlap = find_overlap(existing, new_path);

    match overlap {
        None => None, // No common point, can't merge here
        Some(OverlapInfo {
            existing_node_path,
            new_remainder,
            is_exact_match,
        }) => {
            if is_exact_match && new_remainder.is_empty() {
                // The new path's leaf already exists in the tree
                return Some(MergeResult::InternalNode);
            }

            // Navigate to the overlap point and graft the new nodes
            let target = navigate_to_mut(&mut existing.tree, &existing_node_path);

            if new_remainder.is_empty() {
                // Leaf already exists
                Some(MergeResult::InternalNode)
            } else {
                // Check if this extends an existing branch or creates a new one
                let result = graft_nodes(target, &new_remainder);
                Some(result)
            }
        }
    }
}

struct OverlapInfo {
    /// Path of indices to navigate from the existing root to the overlap node.
    existing_node_path: Vec<usize>,
    /// Remaining new nodes to graft after the overlap point.
    new_remainder: Vec<RevNode>,
    /// Whether the overlap was an exact hash match (vs. positional).
    is_exact_match: bool,
}

/// Find where `new_path` overlaps with `existing`.
fn find_overlap(existing: &RevPath, new_path: &RevPath) -> Option<OverlapInfo> {
    // Flatten the new path into a linear chain of hashes with positions
    let new_chain = flatten_chain(&new_path.tree, new_path.pos);

    // Try to find any node in the new chain that exists in the existing tree
    for (i, (new_pos, new_hash)) in new_chain.iter().enumerate() {
        if let Some(path_indices) = find_node_path(&existing.tree, existing.pos, *new_pos, new_hash)
        {
            // Build the remainder: nodes in the new chain after this overlap point
            let remainder = build_remainder_from_chain(&new_chain, i, &new_path.tree, new_path.pos);

            return Some(OverlapInfo {
                existing_node_path: path_indices,
                new_remainder: remainder,
                is_exact_match: true,
            });
        }
    }

    // Check if the new path starts right after where the existing tree ends,
    // or if there's a positional overlap we can use
    // Check if the new path's root is a child-level continuation of any leaf
    let existing_leaves = collect_leaf_positions(existing);
    let new_root_pos = new_path.pos;
    let new_root_hash = &new_path.tree.hash;

    // Check if the new path starts exactly where an existing leaf is
    for (leaf_pos, leaf_hash, leaf_path) in &existing_leaves {
        // Does the new chain start with this leaf's hash at this position?
        if *leaf_pos == new_root_pos && leaf_hash == new_root_hash {
            let remainder = if new_path.tree.children.is_empty() {
                vec![]
            } else {
                new_path.tree.children.clone()
            };
            return Some(OverlapInfo {
                existing_node_path: leaf_path.clone(),
                new_remainder: remainder,
                is_exact_match: true,
            });
        }
    }

    None
}

/// Flatten a tree node into a linear chain of (pos, hash) pairs.
fn flatten_chain(node: &RevNode, start_pos: u64) -> Vec<(u64, String)> {
    let mut chain = Vec::new();
    fn walk(node: &RevNode, pos: u64, chain: &mut Vec<(u64, String)>) {
        chain.push((pos, node.hash.clone()));
        // Follow first child only (linear chain for the new path)
        if let Some(child) = node.children.first() {
            walk(child, pos + 1, chain);
        }
    }
    walk(node, start_pos, &mut chain);
    chain
}

/// Find the index path to a node with the given position and hash.
fn find_node_path(
    node: &RevNode,
    current_pos: u64,
    target_pos: u64,
    target_hash: &str,
) -> Option<Vec<usize>> {
    if current_pos == target_pos && node.hash == target_hash {
        return Some(vec![]);
    }

    for (i, child) in node.children.iter().enumerate() {
        if let Some(mut path) = find_node_path(child, current_pos + 1, target_pos, target_hash) {
            path.insert(0, i);
            return Some(path);
        }
    }

    None
}

/// Collect all leaf nodes with their positions and index paths.
fn collect_leaf_positions(path: &RevPath) -> Vec<(u64, String, Vec<usize>)> {
    let mut leaves = Vec::new();
    fn walk(
        node: &RevNode,
        pos: u64,
        current_path: &mut Vec<usize>,
        leaves: &mut Vec<(u64, String, Vec<usize>)>,
    ) {
        if node.children.is_empty() {
            leaves.push((pos, node.hash.clone(), current_path.clone()));
        }
        for (i, child) in node.children.iter().enumerate() {
            current_path.push(i);
            walk(child, pos + 1, current_path, leaves);
            current_path.pop();
        }
    }
    let mut current = Vec::new();
    walk(&path.tree, path.pos, &mut current, &mut leaves);
    leaves
}

/// Build the remaining nodes after the overlap point from the new chain.
fn build_remainder_from_chain(
    _chain: &[(u64, String)],
    overlap_index: usize,
    original_tree: &RevNode,
    _original_pos: u64,
) -> Vec<RevNode> {
    // Navigate to the overlap point in the original tree, then return
    // everything after it
    let depth_to_overlap = overlap_index;

    fn get_subtree_at_depth(node: &RevNode, depth: usize) -> Option<&RevNode> {
        if depth == 0 {
            return Some(node);
        }
        if let Some(child) = node.children.first() {
            get_subtree_at_depth(child, depth - 1)
        } else {
            None
        }
    }

    if let Some(overlap_node) = get_subtree_at_depth(original_tree, depth_to_overlap) {
        overlap_node.children.clone()
    } else {
        vec![]
    }
}

/// Navigate to a node in the tree using a path of child indices.
fn navigate_to_mut<'a>(node: &'a mut RevNode, path: &[usize]) -> &'a mut RevNode {
    let mut current = node;
    for &idx in path {
        current = &mut current.children[idx];
    }
    current
}

/// Graft new nodes onto a target node. Returns whether this created a new
/// branch (conflict), extended an existing one, or was a no-op.
fn graft_nodes(target: &mut RevNode, new_nodes: &[RevNode]) -> MergeResult {
    let mut is_new_branch = false;
    let mut added_anything = false;

    for new_node in new_nodes {
        // Check if a child with this hash already exists
        let existing_child = target.children.iter_mut().find(|c| c.hash == new_node.hash);

        match existing_child {
            Some(existing) => {
                // Recursively merge children
                for grandchild in &new_node.children {
                    let sub_nodes = vec![grandchild.clone()];
                    let result = graft_nodes(existing, &sub_nodes);
                    match result {
                        MergeResult::NewBranch => {
                            is_new_branch = true;
                            added_anything = true;
                        }
                        MergeResult::NewLeaf => {
                            added_anything = true;
                        }
                        MergeResult::InternalNode => {}
                    }
                }
            }
            None => {
                // New child — this is either extending a leaf or creating a branch
                if !target.children.is_empty() {
                    is_new_branch = true;
                }
                target.children.push(new_node.clone());
                added_anything = true;
            }
        }
    }

    if !added_anything {
        MergeResult::InternalNode
    } else if is_new_branch {
        MergeResult::NewBranch
    } else {
        MergeResult::NewLeaf
    }
}

// ---------------------------------------------------------------------------
// Winning revision
// ---------------------------------------------------------------------------

/// Determine the winning revision of a document.
///
/// CouchDB's deterministic algorithm:
/// 1. Non-deleted leaves win over deleted leaves
/// 2. Higher position (generation) wins
/// 3. Lexicographically greater hash breaks ties
///
/// Every replica independently arrives at the same winner.
pub fn winning_rev(tree: &RevTree) -> Option<Revision> {
    let leaves = collect_leaves(tree);
    leaves.first().map(|l| Revision::new(l.pos, l.hash.clone()))
}

/// Check if the document's winning revision is deleted.
pub fn is_deleted(tree: &RevTree) -> bool {
    collect_leaves(tree)
        .first()
        .map(|l| l.deleted)
        .unwrap_or(false)
}

/// Collect all conflicting (non-winning, non-deleted) leaf revisions.
pub fn collect_conflicts(tree: &RevTree) -> Vec<Revision> {
    let leaves = collect_leaves(tree);
    leaves
        .iter()
        .skip(1) // skip the winner
        .filter(|l| !l.deleted)
        .map(|l| Revision::new(l.pos, l.hash.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// Stemming (pruning old revisions)
// ---------------------------------------------------------------------------

/// Prune revisions beyond `depth` from each leaf. Returns the list of
/// revision hashes that were removed.
pub fn stem(tree: &mut RevTree, depth: u64) -> Vec<String> {
    let mut stemmed = Vec::new();

    for path in tree.iter_mut() {
        let s = stem_path(path, depth);
        stemmed.extend(s);
    }

    // Remove any paths that became empty
    tree.retain(|p| !is_empty_node(&p.tree));

    stemmed
}

/// Stem a single path, adjusting `pos` if the root gets pruned.
fn stem_path(path: &mut RevPath, depth: u64) -> Vec<String> {
    let mut stemmed = Vec::new();

    // Find the maximum depth of any leaf
    fn max_depth(node: &RevNode) -> u64 {
        if node.children.is_empty() {
            return 0;
        }
        node.children
            .iter()
            .map(|c| 1 + max_depth(c))
            .max()
            .unwrap_or(0)
    }

    let tree_depth = max_depth(&path.tree);

    if tree_depth < depth {
        return stemmed; // Nothing to stem
    }

    // We need to remove nodes from the root until the deepest path
    // is at most `depth` long
    let levels_to_remove = tree_depth - depth + 1;

    for _ in 0..levels_to_remove {
        if path.tree.children.len() <= 1 {
            stemmed.push(path.tree.hash.clone());
            if let Some(child) = path.tree.children.pop() {
                path.tree = child;
                path.pos += 1;
            } else {
                // Tree is now empty
                break;
            }
        } else {
            // Can't stem past a branch point
            break;
        }
    }

    stemmed
}

fn is_empty_node(node: &RevNode) -> bool {
    node.hash.is_empty() && node.children.is_empty()
}

// ---------------------------------------------------------------------------
// Utility: find the latest available revision on a branch
// ---------------------------------------------------------------------------

/// Find the latest available (non-missing) revision following the branch
/// that contains `rev`. If `rev` itself is available, returns it. Otherwise
/// walks toward the leaf looking for the closest available revision.
pub fn latest_rev(tree: &RevTree, pos: u64, hash: &str) -> Option<Revision> {
    for path in tree {
        if let Some(rev) = find_latest_in_node(&path.tree, path.pos, pos, hash) {
            return Some(rev);
        }
    }
    None
}

fn find_latest_in_node(
    node: &RevNode,
    current_pos: u64,
    target_pos: u64,
    target_hash: &str,
) -> Option<Revision> {
    if current_pos == target_pos && node.hash == target_hash {
        // Found the target. If it's available, return it.
        // Otherwise, walk to the first available leaf.
        if node.status == RevStatus::Available {
            return Some(Revision::new(current_pos, node.hash.clone()));
        }
        return find_first_available_leaf(node, current_pos);
    }

    for child in &node.children {
        if let Some(rev) = find_latest_in_node(child, current_pos + 1, target_pos, target_hash) {
            return Some(rev);
        }
    }

    None
}

fn find_first_available_leaf(node: &RevNode, pos: u64) -> Option<Revision> {
    if node.children.is_empty() {
        if node.status == RevStatus::Available {
            return Some(Revision::new(pos, node.hash.clone()));
        }
        return None;
    }

    for child in &node.children {
        if let Some(rev) = find_first_available_leaf(child, pos + 1) {
            return Some(rev);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rev_tree::{NodeOpts, RevNode, RevPath, build_path_from_revs};

    fn leaf(hash: &str) -> RevNode {
        RevNode {
            hash: hash.into(),
            status: RevStatus::Available,
            opts: NodeOpts::default(),
            children: vec![],
        }
    }

    fn deleted_leaf(hash: &str) -> RevNode {
        RevNode {
            hash: hash.into(),
            status: RevStatus::Available,
            opts: NodeOpts { deleted: true },
            children: vec![],
        }
    }

    fn node(hash: &str, children: Vec<RevNode>) -> RevNode {
        RevNode {
            hash: hash.into(),
            status: RevStatus::Available,
            opts: NodeOpts::default(),
            children,
        }
    }

    fn simple_tree() -> RevTree {
        // 1-a -> 2-b -> 3-c
        vec![RevPath {
            pos: 1,
            tree: node("a", vec![node("b", vec![leaf("c")])]),
        }]
    }

    // --- winning_rev ---

    #[test]
    fn winning_rev_simple() {
        let tree = simple_tree();
        let winner = winning_rev(&tree).unwrap();
        assert_eq!(winner.pos, 3);
        assert_eq!(winner.hash, "c");
    }

    #[test]
    fn winning_rev_conflict_picks_higher_hash() {
        // 1-a -> 2-b
        //     -> 2-c
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b"), leaf("c")]),
        }];
        let winner = winning_rev(&tree).unwrap();
        assert_eq!(winner.hash, "c"); // "c" > "b" lexicographically
    }

    #[test]
    fn winning_rev_conflict_prefers_longer() {
        // 1-a -> 2-b -> 3-d
        //     -> 2-c
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![node("b", vec![leaf("d")]), leaf("c")]),
        }];
        let winner = winning_rev(&tree).unwrap();
        assert_eq!(winner.pos, 3);
        assert_eq!(winner.hash, "d"); // pos 3 beats pos 2
    }

    #[test]
    fn winning_rev_non_deleted_beats_deleted() {
        // 1-a -> 2-b (non-deleted)
        //     -> 2-z (deleted) — z > b but deleted loses
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b"), deleted_leaf("z")]),
        }];
        let winner = winning_rev(&tree).unwrap();
        assert_eq!(winner.hash, "b");
    }

    // --- collect_conflicts ---

    #[test]
    fn no_conflicts_on_linear() {
        let tree = simple_tree();
        assert!(collect_conflicts(&tree).is_empty());
    }

    #[test]
    fn conflicts_on_branches() {
        // 1-a -> 2-b, 2-c
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b"), leaf("c")]),
        }];
        let conflicts = collect_conflicts(&tree);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].hash, "b"); // loser
    }

    // --- is_deleted ---

    #[test]
    fn is_deleted_false_for_normal() {
        assert!(!is_deleted(&simple_tree()));
    }

    #[test]
    fn is_deleted_true_when_winner_deleted() {
        let tree = vec![RevPath {
            pos: 1,
            tree: deleted_leaf("a"),
        }];
        assert!(is_deleted(&tree));
    }

    // --- merge_tree ---

    #[test]
    fn merge_extends_linear_chain() {
        // Start: 1-a -> 2-b
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b")]),
        }];

        // Add: 3-c extending from 2-b
        let new_path = build_path_from_revs(
            3,
            &["c".into(), "b".into()],
            NodeOpts::default(),
            RevStatus::Available,
        );

        let (merged, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::NewLeaf);

        let winner = winning_rev(&merged).unwrap();
        assert_eq!(winner.pos, 3);
        assert_eq!(winner.hash, "c");
    }

    #[test]
    fn merge_creates_conflict_branch() {
        // Start: 1-a -> 2-b
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b")]),
        }];

        // Add: 2-c branching from 1-a (conflict)
        let new_path = build_path_from_revs(
            2,
            &["c".into(), "a".into()],
            NodeOpts::default(),
            RevStatus::Available,
        );

        let (merged, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::NewBranch);

        let conflicts = collect_conflicts(&merged);
        assert_eq!(conflicts.len(), 1);
    }

    #[test]
    fn merge_duplicate_is_internal_node() {
        // Start: 1-a -> 2-b
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b")]),
        }];

        // Add: 2-b (already exists)
        let new_path = build_path_from_revs(
            2,
            &["b".into(), "a".into()],
            NodeOpts::default(),
            RevStatus::Available,
        );

        let (_merged, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::InternalNode);
    }

    #[test]
    fn merge_disjoint_creates_new_root() {
        // Start: 1-a -> 2-b
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b")]),
        }];

        // Add: 1-x -> 2-y (completely disjoint)
        let new_path = build_path_from_revs(
            2,
            &["y".into(), "x".into()],
            NodeOpts::default(),
            RevStatus::Available,
        );

        let (merged, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::NewBranch);
        assert_eq!(merged.len(), 2); // Two separate roots
    }

    // --- stem ---

    #[test]
    fn stem_prunes_old_revisions() {
        // 1-a -> 2-b -> 3-c -> 4-d -> 5-e
        let mut tree = vec![RevPath {
            pos: 1,
            tree: node(
                "a",
                vec![node("b", vec![node("c", vec![node("d", vec![leaf("e")])])])],
            ),
        }];

        let stemmed = stem(&mut tree, 3);
        assert!(!stemmed.is_empty());

        // Tree should now start at a higher position
        assert!(tree[0].pos > 1);

        // Leaf should still be present
        let leaves = collect_leaves(&tree);
        assert_eq!(leaves[0].hash, "e");
    }

    #[test]
    fn stem_stops_at_branch_point() {
        // 1-a -> 2-b -> 3-c
        //            -> 3-d
        let mut tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![node("b", vec![leaf("c"), leaf("d")])]),
        }];

        // Even with depth=1, cannot stem past the branch point at 2-b
        let stemmed = stem(&mut tree, 1);
        // Stemmed should remove at most 1-a (stop at branch)
        // Actually, can stem 1-a since 2-b has multiple children
        // but 2-b cannot be stemmed because it has >1 child
        assert!(stemmed.len() <= 1);
    }

    #[test]
    fn stem_short_tree_unchanged() {
        // 1-a -> 2-b (depth 1, limit 3 => nothing to prune)
        let mut tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b")]),
        }];

        let stemmed = stem(&mut tree, 3);
        assert!(stemmed.is_empty());
        assert_eq!(tree[0].pos, 1);
    }

    // --- latest_rev ---

    #[test]
    fn latest_rev_finds_available_node() {
        let tree = simple_tree(); // 1-a -> 2-b -> 3-c
        let rev = latest_rev(&tree, 3, "c").unwrap();
        assert_eq!(rev.pos, 3);
        assert_eq!(rev.hash, "c");
    }

    #[test]
    fn latest_rev_walks_to_leaf_from_missing() {
        // 1-a(missing) -> 2-b(available)
        let tree = vec![RevPath {
            pos: 1,
            tree: RevNode {
                hash: "a".into(),
                status: RevStatus::Missing,
                opts: NodeOpts::default(),
                children: vec![leaf("b")],
            },
        }];
        let rev = latest_rev(&tree, 1, "a").unwrap();
        assert_eq!(rev.pos, 2);
        assert_eq!(rev.hash, "b");
    }

    #[test]
    fn latest_rev_none_for_nonexistent() {
        let tree = simple_tree();
        assert!(latest_rev(&tree, 5, "zzz").is_none());
    }

    #[test]
    fn latest_rev_finds_internal_node() {
        let tree = simple_tree(); // 1-a -> 2-b -> 3-c
        let rev = latest_rev(&tree, 2, "b").unwrap();
        assert_eq!(rev.pos, 2);
        assert_eq!(rev.hash, "b");
    }

    #[test]
    fn latest_rev_on_empty_tree() {
        let tree: RevTree = vec![];
        assert!(latest_rev(&tree, 1, "a").is_none());
    }

    // --- merge edge cases ---

    #[test]
    fn merge_exact_root_match_no_children() {
        // Tree: 1-a (single node)
        let tree = vec![RevPath {
            pos: 1,
            tree: leaf("a"),
        }];

        // Add same node: 1-a
        let new_path = RevPath {
            pos: 1,
            tree: leaf("a"),
        };

        let (_, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::InternalNode);
    }

    #[test]
    fn merge_same_branch_extends_deeper() {
        // Tree: 1-a -> 2-b -> 3-c
        let tree = simple_tree();

        // Add: 1-a -> 2-b -> 3-c -> 4-d (full ancestry extending leaf)
        let new_path = build_path_from_revs(
            4,
            &["d".into(), "c".into(), "b".into(), "a".into()],
            NodeOpts::default(),
            RevStatus::Available,
        );

        let (merged, result) = merge_tree(&tree, &new_path, 1000);
        assert_eq!(result, MergeResult::NewLeaf);
        let winner = winning_rev(&merged).unwrap();
        assert_eq!(winner.pos, 4);
        assert_eq!(winner.hash, "d");
    }

    #[test]
    fn winning_rev_empty_tree() {
        let tree: RevTree = vec![];
        assert!(winning_rev(&tree).is_none());
    }

    #[test]
    fn is_deleted_empty_tree() {
        let tree: RevTree = vec![];
        assert!(!is_deleted(&tree));
    }

    #[test]
    fn collect_conflicts_deleted_leaves_excluded() {
        // 1-a -> 2-b (normal), 2-c (deleted)
        // Winner: 2-b, conflict: none (2-c is deleted)
        let tree = vec![RevPath {
            pos: 1,
            tree: node("a", vec![leaf("b"), deleted_leaf("c")]),
        }];
        let conflicts = collect_conflicts(&tree);
        assert!(conflicts.is_empty());
    }
}
