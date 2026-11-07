//! DBSCAN over points in metres (from alerts-heatmap, T-3333): here the places where vehicles stand
//! still. A grid of `eps` cells finds the neighbours, so tens of thousands of readings stay within a
//! frame or two in the browser.

use std::collections::HashMap;

/// The cluster of each point, `None` for noise; clusters are numbered from 0 in the order found.
pub fn dbscan(points: &[(f64, f64)], eps: f64, min_points: usize) -> Vec<Option<usize>> {
    let mut labels: Vec<Option<usize>> = vec![None; points.len()];
    if points.is_empty() || eps.is_nan() || eps <= 0.0 || min_points == 0 {
        return labels;
    }
    let cell = |(x, y): (f64, f64)| ((x / eps).floor() as i64, (y / eps).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, &p) in points.iter().enumerate() {
        grid.entry(cell(p)).or_default().push(i);
    }
    let neighbours = |i: usize| -> Vec<usize> {
        let (cx, cy) = cell(points[i]);
        let mut found = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid
                    .get(&(cx + dx, cy + dy))
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let (a, b) = (points[i], points[j]);
                    if (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) <= eps * eps {
                        found.push(j);
                    }
                }
            }
        }
        found
    };
    let mut visited = vec![false; points.len()];
    let mut next = 0;
    for i in 0..points.len() {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let around = neighbours(i);
        if around.len() < min_points {
            continue;
        }
        let id = next;
        next += 1;
        labels[i] = Some(id);
        let mut queue = around;
        while let Some(j) = queue.pop() {
            if labels[j].is_none() {
                labels[j] = Some(id);
            }
            if visited[j] {
                continue;
            }
            visited[j] = true;
            let theirs = neighbours(j);
            if theirs.len() >= min_points {
                queue.extend(theirs);
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_dense_places_are_two_clusters_and_a_lone_point_is_noise() {
        let mut points = vec![(0.0, 0.0), (10.0, 0.0), (0.0, 10.0), (5.0, 5.0)];
        points.extend([(1000.0, 1000.0), (1010.0, 1000.0), (1000.0, 1010.0)]);
        points.push((5000.0, 0.0));
        let labels = dbscan(&points, 50.0, 3);
        assert!(labels[..4].iter().all(|l| *l == labels[0] && l.is_some()));
        assert!(labels[4..7].iter().all(|l| *l == labels[4] && l.is_some()));
        assert_ne!(labels[0], labels[4]);
        assert_eq!(labels[7], None);
    }

    #[test]
    fn a_chain_of_neighbours_is_one_cluster_across_grid_cells() {
        let points: Vec<_> = (0..20).map(|i| (f64::from(i) * 40.0, 0.0)).collect();
        let labels = dbscan(&points, 50.0, 2);
        assert!(labels.iter().all(|l| *l == Some(0)), "{labels:?}");
    }

    #[test]
    fn nothing_clusters_from_nothing_or_with_a_radius_of_zero() {
        assert!(dbscan(&[], 50.0, 3).is_empty());
        assert_eq!(dbscan(&[(0.0, 0.0)], 50.0, 2), vec![None]);
        assert_eq!(dbscan(&[(0.0, 0.0), (0.0, 0.0)], 0.0, 1), vec![None, None]);
        // A single point is its own cluster when one point is enough.
        assert_eq!(dbscan(&[(0.0, 0.0)], 50.0, 1), vec![Some(0)]);
    }
}
