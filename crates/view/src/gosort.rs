//! Go 1.26.5 `sort.Slice`: `sort/zsortfunc.go` `pdqsort_func` and its helpers, and `sort/slice.go`.
//!
//! Go's `sort.Slice` is not stable, and the order it leaves equal elements in is observable:
//! `placement.Set.Rank` sorts weight-0 members by id with it and `view.SortNodes` sorts nodes by id, so
//! members or nodes that share an id come out in pdqsort's order. This is a copy of the reviewed port in
//! `dstore_udiff::gosort` (PORTING §3.2 gives `dstore-view` no dependency on `dstore-udiff`), reduced to
//! the unstable sort; a unit test checks the two ports against each other. Go's `int` indices are
//! `isize` here, so every bound that can go negative behaves as in Go.

/// `lessSwap`: the `Less` and `Swap` pair the `_func` variants sort through. Indices are positions in
/// the whole collection, never in a sub-range.
trait LessSwap {
    fn less(&mut self, i: isize, j: isize) -> bool;
    fn swap(&mut self, i: isize, j: isize);
}

/// A slice and a `less` over its elements: what `sort.Slice` builds with `reflectlite.Swapper`.
struct SliceData<'a, T, F> {
    data: &'a mut [T],
    less: F,
}

impl<T, F: FnMut(&T, &T) -> bool> LessSwap for SliceData<'_, T, F> {
    fn less(&mut self, i: isize, j: isize) -> bool {
        (self.less)(&self.data[i as usize], &self.data[j as usize])
    }

    fn swap(&mut self, i: isize, j: isize) {
        self.data.swap(i as usize, j as usize);
    }
}

/// `sort.Slice` = `pdqsort_func(data, 0, n, bits.Len(n))`. Not stable.
pub(crate) fn slice<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool) {
    let length = v.len() as isize;
    let limit = bits_len(v.len());
    pdqsort_func(&mut SliceData { data: v, less }, 0, length, limit);
}

/// `bits.Len(uint(x))`.
fn bits_len(x: usize) -> isize {
    (usize::BITS - x.leading_zeros()) as isize
}

/// `sortedHint`: hint for pdqsort when choosing the pivot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SortedHint {
    Unknown,
    Increasing,
    Decreasing,
}

/// `xorshift` (paper: <https://www.jstatsoft.org/article/view/v008i14/xorshift.pdf>).
struct Xorshift(u64);

impl Xorshift {
    /// `(*xorshift).Next`.
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// `nextPowerOfTwo`.
fn next_power_of_two(length: isize) -> usize {
    let shift = bits_len(length as usize);
    1usize << shift
}

/// `insertionSort_func`: sorts `data[a:b]` using insertion sort.
fn insertion_sort_func<D: LessSwap>(data: &mut D, a: isize, b: isize) {
    let mut i = a + 1;
    while i < b {
        let mut j = i;
        while j > a && data.less(j, j - 1) {
            data.swap(j, j - 1);
            j -= 1;
        }
        i += 1;
    }
}

/// `siftDown_func`: implements the heap property on `data[lo:hi]`. `first` is an offset into the
/// array where the root of the heap lies.
fn sift_down_func<D: LessSwap>(data: &mut D, lo: isize, hi: isize, first: isize) {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            break;
        }
        if child + 1 < hi && data.less(first + child, first + child + 1) {
            child += 1;
        }
        if !data.less(first + root, first + child) {
            return;
        }
        data.swap(first + root, first + child);
        root = child;
    }
}

/// `heapSort_func`.
fn heap_sort_func<D: LessSwap>(data: &mut D, a: isize, b: isize) {
    let first = a;
    let lo = 0;
    let hi = b - a;

    // Build heap with greatest element at top.
    let mut i = (hi - 1) / 2;
    while i >= 0 {
        sift_down_func(data, i, hi, first);
        i -= 1;
    }

    // Pop elements, largest first, into end of data.
    let mut i = hi - 1;
    while i >= 0 {
        data.swap(first, first + i);
        sift_down_func(data, lo, i, first);
        i -= 1;
    }
}

/// `pdqsort_func` sorts `data[a:b]`: pattern-defeating quicksort without the BlockQuicksort
/// optimizations. `limit` is the number of allowed bad (very unbalanced) pivots before falling back
/// to heapsort.
fn pdqsort_func<D: LessSwap>(data: &mut D, mut a: isize, mut b: isize, mut limit: isize) {
    const MAX_INSERTION: isize = 12;

    let mut was_balanced = true; // whether the last partitioning was reasonably balanced
    let mut was_partitioned = true; // whether the slice was already partitioned

    loop {
        let length = b - a;

        if length <= MAX_INSERTION {
            insertion_sort_func(data, a, b);
            return;
        }

        // Fall back to heapsort if too many bad choices were made.
        if limit == 0 {
            heap_sort_func(data, a, b);
            return;
        }

        // If the last partitioning was imbalanced, we need to breaking patterns.
        if !was_balanced {
            break_patterns_func(data, a, b);
            limit -= 1;
        }

        let (mut pivot, mut hint) = choose_pivot_func(data, a, b);
        if hint == SortedHint::Decreasing {
            reverse_range_func(data, a, b);
            // The chosen pivot was pivot-a elements after the start of the array.
            // After reversing it is pivot-a elements before the end of the array.
            pivot = (b - 1) - (pivot - a);
            hint = SortedHint::Increasing;
        }

        // The slice is likely already sorted.
        if was_balanced
            && was_partitioned
            && hint == SortedHint::Increasing
            && partial_insertion_sort_func(data, a, b)
        {
            return;
        }

        // Probably the slice contains many duplicate elements, partition the slice into
        // elements equal to and elements greater than the pivot.
        if a > 0 && !data.less(a - 1, pivot) {
            let mid = partition_equal_func(data, a, b, pivot);
            a = mid;
            continue;
        }

        let (mid, already_partitioned) = partition_func(data, a, b, pivot);
        was_partitioned = already_partitioned;

        let (left_len, right_len) = (mid - a, b - mid);
        let balance_threshold = length / 8;
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort_func(data, a, mid, limit);
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort_func(data, mid + 1, b, limit);
            b = mid;
        }
    }
}

/// `partition_func` does one quicksort partition. With `p = data[pivot]`, it moves the elements of
/// `data[a:b]` so that `data[i] < p` and `data[j] >= p` for `i < newpivot` and `j > newpivot`; on
/// return `data[newpivot] = p`.
fn partition_func<D: LessSwap>(data: &mut D, a: isize, b: isize, pivot: isize) -> (isize, bool) {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    while i <= j && data.less(i, a) {
        i += 1;
    }
    while i <= j && !data.less(j, a) {
        j -= 1;
    }
    if i > j {
        data.swap(j, a);
        return (j, true);
    }
    data.swap(i, j);
    i += 1;
    j -= 1;

    loop {
        while i <= j && data.less(i, a) {
            i += 1;
        }
        while i <= j && !data.less(j, a) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    data.swap(j, a);
    (j, false)
}

/// `partitionEqual_func` partitions `data[a:b]` into elements equal to `data[pivot]` followed by
/// elements greater than `data[pivot]`. It assumes `data[a:b]` has no element smaller than
/// `data[pivot]`.
fn partition_equal_func<D: LessSwap>(data: &mut D, a: isize, b: isize, pivot: isize) -> isize {
    data.swap(a, pivot);
    let (mut i, mut j) = (a + 1, b - 1); // i and j are inclusive of the elements remaining to be partitioned

    loop {
        while i <= j && !data.less(a, i) {
            i += 1;
        }
        while i <= j && data.less(a, j) {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
    i
}

/// `partialInsertionSort_func` partially sorts a slice and reports whether it is sorted at the end.
fn partial_insertion_sort_func<D: LessSwap>(data: &mut D, a: isize, b: isize) -> bool {
    const MAX_STEPS: isize = 5; // maximum number of adjacent out-of-order pairs that will get shifted
    const SHORTEST_SHIFTING: isize = 50; // don't shift any elements on short arrays

    let mut i = a + 1;
    for _ in 0..MAX_STEPS {
        while i < b && !data.less(i, i - 1) {
            i += 1;
        }

        if i == b {
            return true;
        }

        if b - a < SHORTEST_SHIFTING {
            return false;
        }

        data.swap(i, i - 1);

        // Shift the smaller one to the left.
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
                j -= 1;
            }
        }
        // Shift the greater one to the right.
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b {
                if !data.less(j, j - 1) {
                    break;
                }
                data.swap(j, j - 1);
                j += 1;
            }
        }
    }
    false
}

/// `breakPatterns_func` scatters some elements around in an attempt to break patterns that might
/// cause imbalanced partitions in quicksort.
fn break_patterns_func<D: LessSwap>(data: &mut D, a: isize, b: isize) {
    let length = b - a;
    if length >= 8 {
        let mut random = Xorshift(length as u64);
        let modulus = next_power_of_two(length);

        let mut idx = a + (length / 4) * 2 - 1;
        while idx <= a + (length / 4) * 2 + 1 {
            let mut other = ((random.next_u64() as usize) & (modulus - 1)) as isize;
            if other >= length {
                other -= length;
            }
            data.swap(idx, a + other);
            idx += 1;
        }
    }
}

/// `choosePivot_func` chooses a pivot in `data[a:b]`: a static pivot for `[0,8)`, the median of
/// three for `[8,shortestNinther)`, and Tukey's ninther from `shortestNinther` on.
fn choose_pivot_func<D: LessSwap>(data: &mut D, a: isize, b: isize) -> (isize, SortedHint) {
    const SHORTEST_NINTHER: isize = 50;
    const MAX_SWAPS: isize = 4 * 3;

    let l = b - a;

    let mut swaps: isize = 0;
    let mut i = a + l / 4; // a + l/4*1
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;

    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            // Tukey ninther method, the idea came from Rust's implementation.
            i = median_adjacent_func(data, i, &mut swaps);
            j = median_adjacent_func(data, j, &mut swaps);
            k = median_adjacent_func(data, k, &mut swaps);
        }
        // Find the median among i, j, k and stores it into j.
        j = median_func(data, i, j, k, &mut swaps);
    }

    match swaps {
        0 => (j, SortedHint::Increasing),
        MAX_SWAPS => (j, SortedHint::Decreasing),
        _ => (j, SortedHint::Unknown),
    }
}

/// `order2_func` returns `x, y` where `data[x] <= data[y]`, with `x, y` = `a, b` or `b, a`.
fn order2_func<D: LessSwap>(data: &mut D, a: isize, b: isize, swaps: &mut isize) -> (isize, isize) {
    if data.less(b, a) {
        *swaps += 1;
        return (b, a);
    }
    (a, b)
}

/// `median_func` returns `x` where `data[x]` is the median of `data[a]`, `data[b]`, `data[c]`.
fn median_func<D: LessSwap>(
    data: &mut D,
    a: isize,
    b: isize,
    c: isize,
    swaps: &mut isize,
) -> isize {
    let (a, b) = order2_func(data, a, b, swaps);
    let (b, _c) = order2_func(data, b, c, swaps);
    let (_a, b) = order2_func(data, a, b, swaps);
    b
}

/// `medianAdjacent_func` finds the median of `data[a-1]`, `data[a]`, `data[a+1]`.
fn median_adjacent_func<D: LessSwap>(data: &mut D, a: isize, swaps: &mut isize) -> isize {
    median_func(data, a - 1, a, a + 1, swaps)
}

/// `reverseRange_func`.
fn reverse_range_func<D: LessSwap>(data: &mut D, a: isize, b: isize) {
    let mut i = a;
    let mut j = b - 1;
    while i < j {
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splitmix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The copy and `dstore_udiff::gosort::slice` (golden-tested against Go's `sort.Slice` over
    /// `udiff/pdqsort.json`) leave every input in the same order, including under comparators whose
    /// answers depend on the call sequence.
    #[test]
    fn matches_the_udiff_port() {
        let sizes = [
            0usize, 1, 2, 7, 8, 11, 12, 13, 17, 20, 31, 49, 50, 51, 64, 65, 100, 129, 257, 500,
            1000, 2048,
        ];
        let mut state = 42u64;
        for &n in &sizes {
            for modulus in [1u64, 2, 3, 5, 16, 100, u64::MAX] {
                for shape in 0..5 {
                    let v: Vec<(u64, usize)> = (0..n)
                        .map(|i| {
                            let x = match shape {
                                0 => splitmix(&mut state) % modulus,
                                1 => i as u64 / modulus.min(64),
                                2 => (n - 1 - i) as u64 / modulus.min(64),
                                3 => i as u64 % modulus,
                                _ => i.min(n - 1 - i) as u64 / modulus.min(64),
                            };
                            (x, i)
                        })
                        .collect();
                    let (mut ours, mut theirs) = (v.clone(), v);
                    slice(&mut ours, |p, q| p.0 < q.0);
                    dstore_udiff::gosort::slice(&mut theirs, |p, q| p.0 < q.0);
                    assert_eq!(ours, theirs, "n={n} modulus={modulus} shape={shape}");
                }
            }
            // Coin comparators: equal call sequences are needed for equal results; 15/16 exercises
            // breakPatterns and the heapsort fallback.
            for coin in [2u64, 16, 17] {
                let seed = splitmix(&mut state);
                let v: Vec<usize> = (0..n).collect();
                let (mut ours, mut theirs) = (v.clone(), v);
                let (mut s1, mut s2) = (seed, seed);
                let flip = |s: &mut u64| {
                    let c = splitmix(s);
                    if coin == 17 {
                        !c.is_multiple_of(16)
                    } else {
                        c.is_multiple_of(coin)
                    }
                };
                slice(&mut ours, |_, _| flip(&mut s1));
                dstore_udiff::gosort::slice(&mut theirs, |_, _| flip(&mut s2));
                assert_eq!(ours, theirs, "n={n} coin={coin}");
            }
        }
    }
}
