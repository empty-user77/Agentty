//! Split layout of a tab: a tree whose leaves are panes. Generic over the leaf so the tree
//! logic is testable without a window.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    /// Children side by side (split right).
    Horizontal,
    /// Children stacked (split down).
    Vertical,
}

#[derive(Debug, Clone)]
pub enum PaneNode<T> {
    Leaf(T),
    Split { axis: Axis, children: Vec<PaneNode<T>>, sizes: Vec<f32> },
}

pub const MIN_SIZE: f32 = 0.08;

impl<T: Clone + PartialEq> PaneNode<T> {
    pub fn leaves(&self) -> Vec<T> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<T>) {
        match self {
            PaneNode::Leaf(leaf) => out.push(leaf.clone()),
            PaneNode::Split { children, .. } => children.iter().for_each(|c| c.collect(out)),
        }
    }

    pub fn contains(&self, target: &T) -> bool {
        match self {
            PaneNode::Leaf(leaf) => leaf == target,
            PaneNode::Split { children, .. } => children.iter().any(|c| c.contains(target)),
        }
    }

    /// Inserts `new` next to `target`. Splitting in the parent's direction adds a sibling
    /// (so repeated "split down" stacks panes evenly, like cmux); otherwise the leaf is
    /// replaced by a new split.
    pub fn split(&mut self, target: &T, new: T, axis: Axis) -> bool {
        self.attach(target, PaneNode::Leaf(new), axis, false)
    }

    /// Puts a whole subtree next to `target` (a tab dropped into a pane), before it when `before`.
    pub fn attach(&mut self, target: &T, node: PaneNode<T>, axis: Axis, before: bool) -> bool {
        match self {
            PaneNode::Leaf(leaf) if leaf == target => {
                let old = PaneNode::Leaf(leaf.clone());
                let children = if before { vec![node, old] } else { vec![old, node] };
                *self = PaneNode::Split { axis, children, sizes: vec![0.5, 0.5] };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { axis: own_axis, children, sizes } => {
                let direct = children.iter().position(|c| matches!(c, PaneNode::Leaf(l) if l == target));
                if let (Some(index), true) = (direct, *own_axis == axis) {
                    let share = sizes[index] / 2.0;
                    sizes[index] = share;
                    let at = if before { index } else { index + 1 };
                    children.insert(at, node);
                    sizes.insert(at, share);
                    return true;
                }
                let mut node = Some(node);
                children.iter_mut().any(|child| match node.take() {
                    Some(pending) => {
                        if child.attach(target, pending.clone(), axis, before) {
                            true
                        } else {
                            node = Some(pending);
                            false
                        }
                    }
                    None => false,
                })
            }
        }
    }

    /// Removes a leaf. Returns `None` when the tree becomes empty.
    pub fn remove(self, target: &T) -> Option<PaneNode<T>> {
        match self {
            PaneNode::Leaf(leaf) => (leaf != *target).then_some(PaneNode::Leaf(leaf)),
            PaneNode::Split { axis, children, sizes } => {
                let mut kept = Vec::new();
                let mut kept_sizes = Vec::new();
                for (child, size) in children.into_iter().zip(sizes) {
                    if let Some(child) = child.remove(target) {
                        kept.push(child);
                        kept_sizes.push(size);
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    _ => {
                        let total: f32 = kept_sizes.iter().sum();
                        let sizes = kept_sizes.iter().map(|s| s / total).collect();
                        Some(PaneNode::Split { axis, children: kept, sizes })
                    }
                }
            }
        }
    }

    /// The split at `path` (child indices from the root).
    pub fn split_at_mut(&mut self, path: &[usize]) -> Option<&mut PaneNode<T>> {
        match path.split_first() {
            None => Some(self),
            Some((first, rest)) => match self {
                PaneNode::Split { children, .. } => children.get_mut(*first)?.split_at_mut(rest),
                PaneNode::Leaf(_) => None,
            },
        }
    }

    /// Moves the divider between child `index` and `index + 1` by `delta` (fraction of the split).
    pub fn resize(&mut self, path: &[usize], index: usize, start_sizes: &[f32], delta: f32) {
        if let Some(PaneNode::Split { sizes, .. }) = self.split_at_mut(path) {
            if index + 1 >= sizes.len() || start_sizes.len() != sizes.len() {
                return;
            }
            let pair = start_sizes[index] + start_sizes[index + 1];
            let first = (start_sizes[index] + delta).clamp(MIN_SIZE, pair - MIN_SIZE);
            sizes[index] = first;
            sizes[index + 1] = pair - first;
        }
    }

    pub fn map<U>(&self, f: &mut impl FnMut(&T) -> U) -> PaneNode<U> {
        match self {
            PaneNode::Leaf(leaf) => PaneNode::Leaf(f(leaf)),
            PaneNode::Split { axis, children, sizes } => {
                PaneNode::Split { axis: *axis, children: children.iter().map(|c| c.map(f)).collect(), sizes: sizes.clone() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_same_axis_adds_sibling() {
        let mut root = PaneNode::Leaf(1);
        assert!(root.split(&1, 2, Axis::Horizontal));
        assert!(root.split(&2, 3, Axis::Vertical));
        assert!(root.split(&3, 4, Axis::Vertical));
        assert_eq!(root.leaves(), vec![1, 2, 3, 4]);
        match &root {
            PaneNode::Split { children, .. } => match &children[1] {
                PaneNode::Split { axis, children, sizes } => {
                    assert_eq!(*axis, Axis::Vertical);
                    assert_eq!(children.len(), 3);
                    assert!((sizes.iter().sum::<f32>() - 1.0).abs() < 1e-6);
                }
                _ => panic!("expected nested split"),
            },
            _ => panic!("expected split"),
        }
    }

    #[test]
    fn attach_inserts_a_subtree_on_either_side() {
        // A tab with two panes dropped on the left of pane 1.
        let mut root = PaneNode::Leaf(1);
        root.split(&1, 2, Axis::Horizontal);
        let mut moved = PaneNode::Leaf(3);
        moved.split(&3, 4, Axis::Vertical);
        assert!(root.attach(&1, moved, Axis::Horizontal, true));
        assert_eq!(root.leaves(), vec![3, 4, 1, 2]);
        if let PaneNode::Split { sizes, .. } = &root {
            assert!((sizes.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
        // Dropping below turns the leaf into a vertical split, keeping the order.
        let mut root = PaneNode::Leaf(1);
        assert!(root.attach(&1, PaneNode::Leaf(5), Axis::Vertical, false));
        assert_eq!(root.leaves(), vec![1, 5]);
        assert!(!root.attach(&9, PaneNode::Leaf(6), Axis::Vertical, false));
        assert_eq!(root.leaves(), vec![1, 5]);
    }

    #[test]
    fn remove_collapses_and_renormalizes() {
        let mut root = PaneNode::Leaf(1);
        root.split(&1, 2, Axis::Horizontal);
        root.split(&2, 3, Axis::Horizontal);
        let root = root.remove(&2).unwrap();
        assert_eq!(root.leaves(), vec![1, 3]);
        if let PaneNode::Split { sizes, .. } = &root {
            assert!((sizes.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
        let root = root.remove(&1).unwrap();
        assert!(matches!(root, PaneNode::Leaf(3)));
        assert!(root.remove(&3).is_none());
    }

    #[test]
    fn resize_clamps() {
        let mut root = PaneNode::Leaf(1);
        root.split(&1, 2, Axis::Horizontal);
        root.resize(&[], 0, &[0.5, 0.5], 0.9);
        if let PaneNode::Split { sizes, .. } = &root {
            assert!((sizes[0] - (1.0 - MIN_SIZE)).abs() < 1e-6);
            assert!((sizes[1] - MIN_SIZE).abs() < 1e-6);
        }
    }
}
