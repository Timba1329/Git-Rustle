//! Lane algorithm for the commit graph. Expects commits in topological order
//! (children before parents, like `git log --topo-order`).

use crate::model::Commit;

/// A line segment within one row. `from` and `to` are lane indices; in the top half it runs
/// from the top edge (x = from) to the middle (x = to), in the bottom half from the
/// middle (x = from) to the bottom edge (x = to).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Debug, Clone, Default)]
pub struct GraphRow {
    /// Lane of the commit dot.
    pub col: usize,
    pub color: usize,
    pub top: Vec<Seg>,
    pub bottom: Vec<Seg>,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub rows: Vec<GraphRow>,
    /// Maximum number of lanes side by side.
    pub width: usize,
}

type Lanes = Vec<Option<(String, usize)>>;

fn alloc(lanes: &mut Lanes) -> usize {
    if let Some(i) = lanes.iter().position(Option::is_none) {
        i
    } else {
        lanes.push(None);
        lanes.len() - 1
    }
}

/// Free lane closest to `near`; a new lane at the end when all are taken.
fn alloc_near(lanes: &mut Lanes, near: usize) -> usize {
    let best = lanes
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_none())
        .min_by_key(|(i, _)| i.abs_diff(near))
        .map(|(i, _)| i);
    if let Some(i) = best {
        i
    } else {
        lanes.push(None);
        lanes.len() - 1
    }
}

pub fn build_graph(commits: &[Commit]) -> Graph {
    let mut lanes: Lanes = Vec::new();
    let mut next_color = 0usize;
    let mut rows = Vec::with_capacity(commits.len());
    let mut width = 1;

    for c in commits {
        let before = lanes.len();
        let matching: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Some((h, _)) if *h == c.id))
            .map(|(i, _)| i)
            .collect();

        let (col, color) = if let Some(&m) = matching.first() {
            (m, lanes[m].as_ref().map_or(0, |l| l.1))
        } else {
            let i = alloc(&mut lanes);
            next_color += 1;
            (i, next_color - 1)
        };

        let mut row = GraphRow {
            col,
            color,
            ..Default::default()
        };
        for (i, l) in lanes.iter().enumerate() {
            if let Some((h, cl)) = l {
                if *h == c.id {
                    row.top.push(Seg {
                        from: i,
                        to: col,
                        color: *cl,
                    });
                } else {
                    row.top.push(Seg {
                        from: i,
                        to: i,
                        color: *cl,
                    });
                    row.bottom.push(Seg {
                        from: i,
                        to: i,
                        color: *cl,
                    });
                }
            }
        }
        for &m in &matching {
            lanes[m] = None;
        }

        for (pi, p) in c.parents.iter().enumerate() {
            let existing = lanes.iter().position(|l| matches!(l, Some((h, _)) if h == p));
            if let Some(j) = existing {
                let cl = lanes[j].as_ref().map_or(0, |l| l.1);
                row.bottom.push(Seg {
                    from: col,
                    to: j,
                    color: cl,
                });
            } else {
                let (j, cl) = if pi == 0 {
                    (col, color)
                } else {
                    let j = alloc_near(&mut lanes, col);
                    next_color += 1;
                    (j, next_color - 1)
                };
                lanes[j] = Some((p.clone(), cl));
                row.bottom.push(Seg {
                    from: col,
                    to: j,
                    color: cl,
                });
            }
        }

        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        width = width.max(before).max(lanes.len()).max(col + 1);

        // Close gaps: lanes move left into free slots so the graph stays as narrow as possible.
        let mut moved: Vec<(usize, usize)> = Vec::new();
        for j in 0..lanes.len() {
            if lanes[j].is_some()
                && let Some(i) = lanes[..j].iter().position(Option::is_none)
            {
                lanes[i] = lanes[j].take();
                moved.push((j, i));
            }
        }
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        for s in &mut row.bottom {
            if let Some(&(_, n)) = moved.iter().find(|(o, _)| *o == s.to) {
                s.to = n;
            }
        }
        rows.push(row);
    }
    Graph { rows, width }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author_name: String::new(),
            author_email: String::new(),
            time: 0,
            subject: String::new(),
            refs: vec![],
        }
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let g = build_graph(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(g.width, 1);
        assert!(g.rows.iter().all(|r| r.col == 0));
        assert!(g.rows[2].bottom.is_empty());
    }

    #[test]
    fn merge_uses_two_lanes_and_converges() {
        // m = merge(d, e); d en e hebben beide parent b.
        let g = build_graph(&[c("m", &["d", "e"]), c("d", &["b"]), c("e", &["b"]), c("b", &[])]);
        assert_eq!(g.width, 2);
        assert_eq!(g.rows[0].bottom.len(), 2);
        assert_eq!(g.rows[1].col, 0);
        assert_eq!(g.rows[2].col, 1);
        assert!(g.rows[2].bottom.iter().any(|s| s.from == 1 && s.to == 0));
        assert_eq!(g.rows[3].col, 0);
        assert_eq!(g.rows[3].top.len(), 1);
    }

    #[test]
    fn independent_tips_get_separate_lanes() {
        let g = build_graph(&[c("x", &["r"]), c("y", &["r"]), c("r", &[])]);
        assert_eq!(g.rows[0].col, 0);
        assert_eq!(g.rows[1].col, 1);
        assert!(g.rows[1].bottom.iter().any(|s| s.from == 1 && s.to == 0));
        assert_eq!(g.rows[2].col, 0);
        assert_eq!(g.rows[2].top.len(), 1);
    }
}
