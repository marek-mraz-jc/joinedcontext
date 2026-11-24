//! Go's `sort.Slice` (pdqsort, `src/sort/zsortfunc.go` of Go 1.27, which Bento 1.22.0 is built
//! with), ported step for step. It is not stable: `sort` and `sort_by` must reorder equal elements
//! exactly as Bento does, which a stable sort does not.

/// Sorts `v` as Go's `sort.Slice(v, less)` does.
pub fn sort_slice<T>(v: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    sort_slice_at(v, |_, a, _, b| less(a, b));
}

/// [`sort_slice`] whose `less` also gets both elements' current places, as Go's `less(i, j)` does.
pub fn sort_slice_at<T>(v: &mut [T], mut less: impl FnMut(usize, &T, usize, &T) -> bool) {
    let n = v.len();
    let limit = usize::BITS - n.leading_zeros();
    let mut d = Data { v, less: &mut less };
    pdqsort(&mut d, 0, n, limit);
}

struct Data<'a, T, F: FnMut(usize, &T, usize, &T) -> bool> {
    v: &'a mut [T],
    less: &'a mut F,
}

impl<T, F: FnMut(usize, &T, usize, &T) -> bool> Data<'_, T, F> {
    fn less(&mut self, i: usize, j: usize) -> bool {
        (self.less)(i, &self.v[i], j, &self.v[j])
    }
    fn swap(&mut self, i: usize, j: usize) {
        self.v.swap(i, j);
    }
}

#[derive(PartialEq)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}

fn insertion_sort<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) {
    for i in a + 1..b {
        let mut j = i;
        while j > a && d.less(j, j - 1) {
            d.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn sift_down<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    lo: usize,
    hi: usize,
    first: usize,
) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            return;
        }
        if child + 1 < hi && d.less(first + child, first + child + 1) {
            child += 1;
        }
        if !d.less(first + root, first + child) {
            return;
        }
        d.swap(first + root, first + child);
        root = child;
    }
}

fn heap_sort<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) {
    let first = a;
    let hi = b - a;
    for i in (0..hi / 2).rev() {
        sift_down(d, i, hi, first);
    }
    for i in (0..hi).rev() {
        d.swap(first, first + i);
        sift_down(d, 0, i, first);
    }
}

fn pdqsort<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    mut a: usize,
    mut b: usize,
    mut limit: u32,
) {
    const MAX_INSERTION: usize = 12;
    let mut was_balanced = true;
    let mut was_partitioned = true;
    loop {
        let length = b - a;
        if length <= MAX_INSERTION {
            insertion_sort(d, a, b);
            return;
        }
        if limit == 0 {
            heap_sort(d, a, b);
            return;
        }
        if !was_balanced {
            break_patterns(d, a, b);
            limit -= 1;
        }
        let (mut pivot, mut hint) = choose_pivot(d, a, b);
        if hint == Hint::Decreasing {
            reverse_range(d, a, b);
            pivot = (b - 1) - (pivot - a);
            hint = Hint::Increasing;
        }
        if was_balanced
            && was_partitioned
            && hint == Hint::Increasing
            && partial_insertion_sort(d, a, b)
        {
            return;
        }
        if a > 0 && !d.less(a - 1, pivot) {
            a = partition_equal(d, a, b, pivot);
            continue;
        }
        let (mid, already) = partition(d, a, b, pivot);
        was_partitioned = already;
        let (left, right) = (mid - a, b - mid);
        let balance = length / 8;
        if left < right {
            was_balanced = left >= balance;
            pdqsort(d, a, mid, limit);
            a = mid + 1;
        } else {
            was_balanced = right >= balance;
            pdqsort(d, mid + 1, b, limit);
            b = mid;
        }
    }
}

/// Go's `partition_func`; `i` and `j` are signed there, `j` may reach `a` (never below).
fn partition<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
    pivot: usize,
) -> (usize, bool) {
    d.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    while i <= j && d.less(i, a) {
        i += 1;
    }
    while i <= j && !d.less(j, a) {
        j -= 1;
    }
    if i > j {
        d.swap(j, a);
        return (j, true);
    }
    d.swap(i, j);
    i += 1;
    j -= 1;
    loop {
        while i <= j && d.less(i, a) {
            i += 1;
        }
        while i <= j && !d.less(j, a) {
            j -= 1;
        }
        if i > j {
            break;
        }
        d.swap(i, j);
        i += 1;
        j -= 1;
    }
    d.swap(j, a);
    (j, false)
}

fn partition_equal<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
    pivot: usize,
) -> usize {
    d.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1);
    loop {
        while i <= j && !d.less(a, i) {
            i += 1;
        }
        while i <= j && d.less(a, j) {
            j -= 1;
        }
        if i > j {
            break;
        }
        d.swap(i, j);
        i += 1;
        j -= 1;
    }
    i
}

fn partial_insertion_sort<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) -> bool {
    const MAX_STEPS: usize = 5;
    const SHORTEST_SHIFTING: usize = 50;
    let mut i = a + 1;
    for _ in 0..MAX_STEPS {
        while i < b && !d.less(i, i - 1) {
            i += 1;
        }
        if i == b {
            return true;
        }
        if b - a < SHORTEST_SHIFTING {
            return false;
        }
        d.swap(i, i - 1);
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !d.less(j, j - 1) {
                    break;
                }
                d.swap(j, j - 1);
                j -= 1;
            }
        }
        for j in i + 1..b {
            if !d.less(j, j - 1) {
                break;
            }
            d.swap(j, j - 1);
        }
    }
    false
}

fn break_patterns<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) {
    // pdqsort calls this only past MAX_INSERTION (12), so Go's `length >= 8` test always holds.
    let length = b - a;
    let mut random = length as u64;
    let modulus = 1usize << (usize::BITS - length.leading_zeros());
    let mid = a + (length / 4) * 2;
    for idx in mid - 1..=mid + 1 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let mut other = (random as usize) & (modulus - 1);
        if other >= length {
            other -= length;
        }
        d.swap(idx, a + other);
    }
}

fn choose_pivot<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) -> (usize, Hint) {
    const SHORTEST_NINTHER: usize = 50;
    const MAX_SWAPS: usize = 4 * 3;
    let l = b - a;
    let mut swaps = 0;
    let (mut i, mut j, mut k) = (a + l / 4, a + l / 4 * 2, a + l / 4 * 3);
    // l > MAX_INSERTION (12) here, so Go's `l >= 8` test always holds.
    if l >= SHORTEST_NINTHER {
        i = median(d, i - 1, i, i + 1, &mut swaps);
        j = median(d, j - 1, j, j + 1, &mut swaps);
        k = median(d, k - 1, k, k + 1, &mut swaps);
    }
    j = median(d, i, j, k, &mut swaps);
    match swaps {
        0 => (j, Hint::Increasing),
        MAX_SWAPS => (j, Hint::Decreasing),
        _ => (j, Hint::Unknown),
    }
}

fn order2<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
    swaps: &mut usize,
) -> (usize, usize) {
    if d.less(b, a) {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

fn median<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
    c: usize,
    swaps: &mut usize,
) -> usize {
    let (a, b) = order2(d, a, b, swaps);
    let (b, c) = order2(d, b, c, swaps);
    let (_a, b) = order2(d, a, b, swaps);
    let _ = c;
    b
}

fn reverse_range<T, F: FnMut(usize, &T, usize, &T) -> bool>(
    d: &mut Data<'_, T, F>,
    a: usize,
    b: usize,
) {
    let (mut i, mut j) = (a, b - 1);
    while i < j {
        d.swap(i, j);
        i += 1;
        j -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{heap_sort, partial_insertion_sort, pdqsort, sort_slice, Data};

    #[test]
    fn equal_keys_land_where_go_puts_them() {
        // Recorded with Go 1.27 sort.Slice over (key, index) by key: 13+ elements leave insertion
        // sort, so equal keys no longer keep their input order.
        let keys = [3, 1, 2, 1, 3, 2, 1, 3, 2, 1, 3, 2, 1, 3, 2, 1, 3, 2, 1, 3];
        let mut v: Vec<(i32, usize)> = keys.iter().copied().zip(0..).collect();
        sort_slice(&mut v, |a, b| a.0 < b.0);
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
        let order: Vec<usize> = v.iter().map(|p| p.1).collect();
        assert_eq!(order, GO_ORDER);
    }

    // Filled from Bento: `range(0,20).map_each(i -> {"k": [3,1,2,1,3,2,1,3,2,1,3,2,1,3,2,1,3,2,1,3].index(i), "i": i}).sort_by(e -> e.k).map_each(e -> e.i)`.
    const GO_ORDER: [usize; 20] = [
        9, 1, 18, 3, 15, 12, 6, 5, 8, 11, 14, 17, 2, 7, 10, 0, 13, 4, 16, 19,
    ];

    #[test]
    fn sorts_large_inputs() {
        let mut v: Vec<u64> = (0..5000u64).map(|i| (i * 7919) % 1013).collect();
        sort_slice(&mut v, |a, b| a < b);
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
        let mut r: Vec<u64> = (0..300).rev().collect();
        sort_slice(&mut r, |a, b| a < b);
        assert_eq!(r, (0..300).collect::<Vec<_>>());
    }

    fn assert_sorts(name: &str, mut v: Vec<i64>) {
        let mut want = v.clone();
        want.sort_unstable();
        sort_slice(&mut v, |a, b| a < b);
        assert_eq!(v, want, "{name}");
    }

    #[test]
    fn sorts_every_input_shape_pdqsort_branches_on() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = |m: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m) as i64
        };
        for n in [13usize, 49, 50, 64, 100, 257, 1000, 4096] {
            assert_sorts("random", (0..n).map(|_| rnd(1 << 20)).collect());
            assert_sorts("few unique", (0..n).map(|_| rnd(3)).collect());
            assert_sorts("all equal", vec![7; n]);
            assert_sorts("ascending", (0..n as i64).collect());
            assert_sorts("descending", (0..n as i64).rev().collect());
            assert_sorts("sawtooth", (0..n as i64).map(|i| i % 17).collect());
            assert_sorts(
                "organ pipe",
                (0..n as i64).map(|i| i.min(n as i64 - i)).collect(),
            );
            let mut nearly: Vec<i64> = (0..n as i64).collect();
            nearly.swap(1, 2);
            nearly.swap(n - 3, n - 2);
            assert_sorts("nearly sorted", nearly.clone());
            nearly.reverse();
            assert_sorts("nearly descending", nearly);
            let mut shifted: Vec<i64> = (0..n as i64).collect();
            shifted.rotate_left(1);
            assert_sorts("rotated", shifted);
            let mut one_out: Vec<i64> = (0..n as i64).collect();
            one_out[n / 2] = -1;
            assert_sorts("one out of place", one_out);
            assert_sorts(
                "median of three killer",
                (0..n as i64)
                    .map(|i| if i % 2 == 0 { i } else { n as i64 + i })
                    .collect(),
            );
        }
    }

    #[test]
    fn the_number_of_comparisons_is_pinned_per_input_shape() {
        // sort_slice_at's less sees places, as Go's less(i, j) does, so the comparison sequence is
        // observable; a different recursion limit or pivot rule changes these counts.
        let want = [
            (100, [632, 605, 677, 630]),
            (1000, [9819, 5763, 8289, 9690]),
            (4096, [47656, 21959, 34253, 47165]),
        ];
        for (n, counts) in want {
            for (shape, count) in counts.into_iter().enumerate() {
                let mut v: Vec<i64> = (0..n as i64)
                    .map(|i| match shape {
                        0 => {
                            if i % 2 == 0 {
                                i
                            } else {
                                n as i64 + i
                            }
                        }
                        1 => i % 17,
                        2 => (i * 7919) % 101,
                        _ => i.min(n as i64 - i),
                    })
                    .collect();
                let mut calls = 0;
                sort_slice(&mut v, |a, b| {
                    calls += 1;
                    a < b
                });
                assert_eq!(calls, count, "n {n} shape {shape}");
                assert!(v.windows(2).all(|w| w[0] <= w[1]));
            }
        }
    }

    #[test]
    fn a_spent_recursion_limit_falls_back_to_heap_sort() {
        for n in [13usize, 40, 101] {
            let mut v: Vec<i64> = (0..n as i64).map(|i| (i * 37) % 23).collect();
            let mut want = v.clone();
            want.sort_unstable();
            let mut less = |_: usize, a: &i64, _: usize, b: &i64| a < b;
            let mut d = Data {
                v: &mut v,
                less: &mut less,
            };
            pdqsort(&mut d, 0, n, 0);
            assert_eq!(v, want);
        }
    }

    #[test]
    fn sorted_and_descending_inputs_take_their_counted_paths() {
        // Sorted input ends in partial_insertion_sort; descending input is reversed around its
        // pivot first. Counts pinned like the shapes above.
        let mut counts = Vec::new();
        for n in [100usize, 1000] {
            for shape in 0..3 {
                let mut v: Vec<i64> = (0..n as i64)
                    .map(|i| match shape {
                        0 => i,
                        1 => n as i64 - i,
                        _ => n as i64 - i + if i % 7 == 0 { 3 } else { 0 },
                    })
                    .collect();
                let mut calls = 0usize;
                sort_slice(&mut v, |a, b| {
                    calls += 1;
                    a < b
                });
                assert!(v.windows(2).all(|w| w[0] <= w[1]));
                counts.push(calls);
            }
        }
        assert_eq!(counts, vec![111, 111, 565, 1011, 1011, 9485]);
    }

    #[test]
    fn an_unbalanced_partition_spends_the_recursion_limit() {
        // With limit 1, one unbalanced partition leaves 0 and the next round falls back to
        // heap_sort, which compares differently from another pdqsort round.
        let count = |shape: usize, limit: u32| {
            let n = 400i64;
            let mut v: Vec<i64> = (0..n)
                .map(|i| match shape {
                    0 => (i * 7919) % 401,
                    1 => i.min(n - i),
                    2 => {
                        if i % 50 == 0 {
                            0
                        } else {
                            n - i % 3
                        }
                    }
                    _ => (i * i) % 397,
                })
                .collect();
            let mut calls = 0usize;
            let mut less = |_: usize, a: &i64, _: usize, b: &i64| {
                calls += 1;
                a < b
            };
            let mut d = Data {
                v: &mut v,
                less: &mut less,
            };
            pdqsort(&mut d, 0, n as usize, limit);
            assert!(v.windows(2).all(|w| w[0] <= w[1]));
            calls
        };
        let probe: Vec<_> = (0..4).map(|s| (count(s, 1), count(s, 64))).collect();
        assert_eq!(
            probe,
            vec![(3618, 3583), (3296, 3282), (3253, 1522), (3437, 3437)]
        );
    }

    #[test]
    fn partial_insertion_sort_shifts_from_fifty_elements_on() {
        // One swapped pair: 50 elements are short enough to fix in place, 49 give up at once.
        for (n, fixed) in [(50i64, true), (49, false)] {
            let mut v: Vec<i64> = (0..n).collect();
            v.swap(10, 11);
            let mut less = |_: usize, a: &i64, _: usize, b: &i64| a < b;
            let mut d = Data {
                v: &mut v,
                less: &mut less,
            };
            assert_eq!(
                partial_insertion_sort(&mut d, 0, n as usize),
                fixed,
                "n {n}"
            );
            assert_eq!(v.windows(2).all(|w| w[0] <= w[1]), fixed, "n {n}");
        }
    }

    #[test]
    fn heap_sort_sorts_only_its_sub_range() {
        // pdqsort hands heap_sort a sub-range [a, b) once its limit is spent; the ends stay put.
        let mut v: Vec<i64> = vec![99, 98, 5, 3, 9, 1, 7, 4, -1, -2];
        let mut less = |_: usize, a: &i64, _: usize, b: &i64| a < b;
        let mut d = Data {
            v: &mut v,
            less: &mut less,
        };
        heap_sort(&mut d, 2, 8);
        assert_eq!(v, vec![99, 98, 1, 3, 4, 5, 7, 9, -1, -2]);
    }
}
