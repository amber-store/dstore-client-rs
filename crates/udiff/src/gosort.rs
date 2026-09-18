//! Go 1.26.5 `sort`: `sort/zsortfunc.go` (`pdqsort_func` and helpers), `sort/sort.go:59-80`,
//! `sort/slice.go`, and `sort.Stable`.
//!
//! Ported line by line. [`slice`] is Go's unstable pattern-defeating quicksort: the order it leaves
//! equal elements in is observable (go-udiff's `lcs.fix` sorts diagonals by length with it), so it
//! must never be replaced by another sort. Go's `int` indices are `isize` here, so every bound that
//! can go negative behaves as in Go.

/// `lessSwap`: the `Less` and `Swap` pair the `_func` variants sort through. Indices are positions in
/// the whole collection, never in a sub-range.
pub(crate) trait LessSwap {
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
pub fn slice<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool) {
    let length = v.len() as isize;
    let limit = bits_len(v.len());
    pdqsort_func(&mut SliceData { data: v, less }, 0, length, limit);
}

/// `sort.SliceStable` / `sort.Stable` (insertion sort + symMerge).
pub fn slice_stable<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool) {
    let n = v.len() as isize;
    stable_func(&mut SliceData { data: v, less }, n);
}

/// `sort.SliceIsSorted` (and `sort.IsSorted`): scans from the end for `less(v[i], v[i-1])`.
pub fn is_sorted<T>(v: &[T], mut less: impl FnMut(&T, &T) -> bool) -> bool {
    let n = v.len() as isize;
    let mut i = n - 1;
    while i > 0 {
        if less(&v[i as usize], &v[(i - 1) as usize]) {
            return false;
        }
        i -= 1;
    }
    true
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

/// `swapRange_func`.
fn swap_range_func<D: LessSwap>(data: &mut D, a: isize, b: isize, n: isize) {
    for i in 0..n {
        data.swap(a + i, b + i);
    }
}

/// `stable_func`.
fn stable_func<D: LessSwap>(data: &mut D, n: isize) {
    let mut block_size = 20; // must be > 0
    let (mut a, mut b) = (0, block_size);
    while b <= n {
        insertion_sort_func(data, a, b);
        a = b;
        b += block_size;
    }
    insertion_sort_func(data, a, n);

    while block_size < n {
        (a, b) = (0, 2 * block_size);
        while b <= n {
            sym_merge_func(data, a, a + block_size, b);
            a = b;
            b += 2 * block_size;
        }
        let m = a + block_size;
        if m < n {
            sym_merge_func(data, a, m, n);
        }
        block_size *= 2;
    }
}

/// `int(uint(x) >> 1)`.
fn half(x: isize) -> isize {
    ((x as usize) >> 1) as isize
}

/// `symMerge_func` merges the two sorted subsequences `data[a:m]` and `data[m:b]` using the SymMerge
/// algorithm of Pok-Son Kim and Arne Kutzner ("Stable Minimum Storage Merging by Symmetric
/// Comparisons", ESA 2004). It assumes non-degenerate arguments: `a < m && m < b`.
fn sym_merge_func<D: LessSwap>(data: &mut D, a: isize, m: isize, b: isize) {
    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[a] into data[m:b]
    // if data[a:m] only contains one element.
    if m - a == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] >= data[a] for m <= i < b.
        // Exit the search loop with i == b in case no such index exists.
        let mut i = m;
        let mut j = b;
        while i < j {
            let h = half(i + j);
            if data.less(h, a) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[a] reaches the position before i.
        let mut k = a;
        while k < i - 1 {
            data.swap(k, k + 1);
            k += 1;
        }
        return;
    }

    // Avoid unnecessary recursions of symMerge
    // by direct insertion of data[m] into data[a:m]
    // if data[m:b] only contains one element.
    if b - m == 1 {
        // Use binary search to find the lowest index i
        // such that data[i] > data[m] for a <= i < m.
        // Exit the search loop with i == m in case no such index exists.
        let mut i = a;
        let mut j = m;
        while i < j {
            let h = half(i + j);
            if !data.less(m, h) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        // Swap values until data[m] reaches the position i.
        let mut k = m;
        while k > i {
            data.swap(k, k - 1);
            k -= 1;
        }
        return;
    }

    let mid = half(a + b);
    let n = mid + m;
    let (mut start, mut r) = if m > mid { (n - b, mid) } else { (a, m) };
    let p = n - 1;

    while start < r {
        let c = half(start + r);
        if !data.less(p - c, c) {
            start = c + 1;
        } else {
            r = c;
        }
    }

    let end = n - start;
    if start < m && m < end {
        rotate_func(data, start, m, end);
    }
    if a < start && start < mid {
        sym_merge_func(data, a, start, mid);
    }
    if mid < end && end < b {
        sym_merge_func(data, mid, end, b);
    }
}

/// `rotate_func` rotates two consecutive blocks `u = data[a:m]` and `v = data[m:b]`: `x u v y`
/// becomes `x v u y`. It assumes non-degenerate arguments: `a < m && m < b`.
fn rotate_func<D: LessSwap>(data: &mut D, a: isize, m: isize, b: isize) {
    let mut i = m - a;
    let mut j = b - m;

    while i != j {
        if i > j {
            swap_range_func(data, m - i, m, j);
            i -= j;
        } else {
            swap_range_func(data, m - i, m + j - i, i);
            j -= i;
        }
    }
    // i == j
    swap_range_func(data, m - i, m, i);
}

/// Deterministic pseudo-random numbers for tests: splitmix64 as in VECTORS.md.
#[cfg(test)]
pub(crate) mod testrng {
    pub(crate) struct Rng(pub(crate) u64);

    impl Rng {
        pub(crate) fn next_u64(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// A number in `[0, n)`.
        pub(crate) fn intn(&mut self, n: usize) -> usize {
            (self.next_u64() % n as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    //! Ports of Go 1.26.5 `sort/sort_test.go` (`TestSortIntSlice`, `TestSortLarge_Random`,
    //! `TestReverseSortIntSlice`, `TestBreakPatterns`, `TestReverseRange`,
    //! `TestNonDeterministicComparison`, `TestSortBM`, `TestHeapsortBM`, `TestStableBM`,
    //! `TestAdversary`, `TestStableInts`, `TestStability`), with splitmix64 instead of math/rand.
    //! The exact permutations `slice` produces are locked by `udiff/pdqsort.json`.

    use super::testrng::Rng;
    use super::*;

    const INTS: [i64; 13] = [
        74, 59, 238, -784, 9845, 959, 905, 0, 0, 42, 7586, -5467984, 7586,
    ];

    fn ints_are_sorted(v: &[i64]) -> bool {
        v.windows(2).all(|w| w[0] <= w[1])
    }

    /// `sort.Sort`.
    fn sort<D: LessSwap>(data: &mut D, n: isize) {
        if n <= 1 {
            return;
        }
        let limit = bits_len(n as usize);
        pdqsort_func(data, 0, n, limit);
    }

    /// `sort.Heapsort` of `export_test.go`.
    fn heapsort<D: LessSwap>(data: &mut D, n: isize) {
        heap_sort_func(data, 0, n);
    }

    /// `sort.Stable`.
    fn stable<D: LessSwap>(data: &mut D, n: isize) {
        stable_func(data, n);
    }

    /// `lg`.
    fn lg(n: usize) -> usize {
        let mut i = 0;
        while (1usize << i) < n {
            i += 1;
        }
        i
    }

    /// `testingData`: counts comparisons and fails when more than `maxswap` swaps are used.
    struct TestingData {
        desc: String,
        data: Vec<i64>,
        maxswap: usize,
        ncmp: usize,
        nswap: usize,
    }

    impl LessSwap for TestingData {
        fn less(&mut self, i: isize, j: isize) -> bool {
            self.ncmp += 1;
            self.data[i as usize] < self.data[j as usize]
        }

        fn swap(&mut self, i: isize, j: isize) {
            assert!(
                self.nswap < self.maxswap,
                "{}: used {} swaps sorting slice of {}",
                self.desc,
                self.nswap,
                self.data.len()
            );
            self.nswap += 1;
            self.data.swap(i as usize, j as usize);
        }
    }

    #[test]
    fn sort_int_slice() {
        let mut data = INTS;
        slice(&mut data, |a, b| a < b);
        assert!(ints_are_sorted(&data), "sorted {INTS:?}\n   got {data:?}");
        assert!(is_sorted(&data, |a, b| a < b));
    }

    #[test]
    fn stable_ints() {
        let mut data = INTS;
        slice_stable(&mut data, |a, b| a < b);
        assert!(ints_are_sorted(&data), "nsorted {INTS:?}\n   got {data:?}");
    }

    #[test]
    fn sort_large_random() {
        let mut rng = Rng(1);
        let mut data: Vec<i64> = (0..100_000).map(|_| rng.intn(100) as i64).collect();
        assert!(!ints_are_sorted(&data), "terrible rand.rand");
        slice(&mut data, |a, b| a < b);
        assert!(ints_are_sorted(&data), "sort didn't sort - 100K ints");
    }

    #[test]
    fn reverse_sort_int_slice() {
        let mut a = INTS;
        let mut r = INTS;
        slice(&mut a, |x, y| x < y);
        slice(&mut r, |x, y| y < x); // sort.Reverse
        for i in 0..a.len() {
            assert_eq!(a[i], r[a.len() - 1 - i], "reverse sort didn't sort");
            if i > a.len() / 2 {
                break;
            }
        }
    }

    #[test]
    fn break_patterns() {
        // Special slice used to trigger breakPatterns.
        let mut data = [10i64; 30];
        data[30 / 4] = 0;
        data[(30 / 4) * 2] = 1;
        data[(30 / 4) * 3] = 2;
        slice(&mut data, |a, b| a < b);
        assert!(ints_are_sorted(&data), "{data:?}");
    }

    #[test]
    fn reverse_range() {
        let mut data = SliceData {
            data: &mut [1i64, 2, 3, 4, 5, 6, 7][..],
            less: |a: &i64, b: &i64| a < b,
        };
        reverse_range_func(&mut data, 0, 7);
        assert_eq!(data.data, [7, 6, 5, 4, 3, 2, 1], "reverseRange didn't work");

        let mut data1 = SliceData {
            data: &mut [1i64, 2, 3, 4, 5, 6, 7][..],
            less: |a: &i64, b: &i64| a < b,
        };
        reverse_range_func(&mut data1, 2, 5);
        assert_eq!(
            data1.data,
            [1, 2, 5, 4, 3, 6, 7],
            "reverseRange didn't work"
        );
    }

    /// `nonDeterministicTestingData`.
    struct NonDeterministic(Rng);

    impl LessSwap for NonDeterministic {
        fn less(&mut self, i: isize, j: isize) -> bool {
            assert!(
                (0..500).contains(&i) && (0..500).contains(&j),
                "nondeterministic comparison out of bounds"
            );
            self.0.next_u64() & 1 == 0
        }

        fn swap(&mut self, i: isize, j: isize) {
            assert!(
                (0..500).contains(&i) && (0..500).contains(&j),
                "nondeterministic comparison out of bounds"
            );
        }
    }

    #[test]
    fn non_deterministic_comparison() {
        // sort.Sort does not panic when Less returns inconsistent results (golang.org/issue/14377).
        let mut td = NonDeterministic(Rng(0));
        for _ in 0..10 {
            sort(&mut td, 500);
        }
    }

    const DISTS: [&str; 5] = ["sawtooth", "rand", "stagger", "plateau", "shuffle"];
    const MODES: [&str; 6] = ["copy", "reverse", "reverse1", "reverse2", "sort", "dither"];

    /// `testBentleyMcIlroy`, over the short and the full size lists.
    fn bentley_mcilroy(algo: fn(&mut TestingData, isize), maxswap: fn(usize) -> usize) {
        let mut rng = Rng(7);
        for n in [100usize, 127, 128, 129, 1023, 1024, 1025] {
            let mut m = 1;
            while m < 2 * n {
                for (dist, dist_name) in DISTS.iter().enumerate() {
                    let (mut j, mut k) = (0i64, 1i64);
                    let data: Vec<i64> = (0..n)
                        .map(|i| match dist {
                            0 => (i % m) as i64,
                            1 => rng.intn(m) as i64,
                            2 => ((i * m + i) % n) as i64,
                            3 => i.min(m) as i64,
                            _ => {
                                if rng.intn(m) != 0 {
                                    j += 2;
                                    j
                                } else {
                                    k += 2;
                                    k
                                }
                            }
                        })
                        .collect();

                    for (mode, mode_name) in MODES.iter().enumerate() {
                        let mdata: Vec<i64> = match mode {
                            0 => data.clone(),
                            1 => data.iter().rev().copied().collect(),
                            2 => (0..n)
                                .map(|i| {
                                    if i < n / 2 {
                                        data[n / 2 - i - 1]
                                    } else {
                                        data[i]
                                    }
                                })
                                .collect(),
                            3 => (0..n)
                                .map(|i| {
                                    if i < n / 2 {
                                        data[i]
                                    } else {
                                        data[n - (i - n / 2) - 1]
                                    }
                                })
                                .collect(),
                            4 => {
                                let mut v = data.clone();
                                v.sort_unstable();
                                v
                            }
                            _ => data
                                .iter()
                                .enumerate()
                                .map(|(i, v)| v + (i % 5) as i64)
                                .collect(),
                        };

                        let mut d = TestingData {
                            desc: format!("n={n} m={m} dist={dist_name} mode={mode_name}"),
                            data: mdata,
                            maxswap: maxswap(n),
                            ncmp: 0,
                            nswap: 0,
                        };
                        algo(&mut d, n as isize);
                        assert!(
                            ints_are_sorted(&d.data),
                            "{}: ints not sorted\n\t{:?}",
                            d.desc,
                            d.data
                        );
                        assert!(d.ncmp > 0 || n <= 1, "{}: no comparisons", d.desc);
                    }
                }
                m *= 2;
            }
        }
    }

    #[test]
    fn sort_bm() {
        bentley_mcilroy(sort, |n| n * lg(n) * 12 / 10);
    }

    #[test]
    fn heapsort_bm() {
        bentley_mcilroy(heapsort, |n| n * lg(n) * 12 / 10);
    }

    #[test]
    fn stable_bm() {
        bentley_mcilroy(stable, |n| n * lg(n) * lg(n) / 3);
    }

    /// `adversaryTestingData`: M. Douglas McIlroy's "antiquicksort"
    /// (<https://www.cs.dartmouth.edu/~doug/mdmspe.pdf>).
    struct AdversaryTestingData {
        data: Vec<isize>, // item values, initialized to special gas value and changed by Less
        maxcmp: usize,    // number of comparisons allowed
        ncmp: usize,      // number of comparisons (calls to Less)
        nsolid: isize,    // number of elements that have been set to non-gas values
        candidate: isize, // guess at current pivot
        gas: isize,       // special value for unset elements, higher than everything else
    }

    impl LessSwap for AdversaryTestingData {
        fn less(&mut self, i: isize, j: isize) -> bool {
            assert!(
                self.ncmp < self.maxcmp,
                "used {} comparisons sorting adversary data with size {}",
                self.ncmp,
                self.data.len()
            );
            self.ncmp += 1;

            let (iu, ju) = (i as usize, j as usize);
            if self.data[iu] == self.gas && self.data[ju] == self.gas {
                if i == self.candidate {
                    // freeze i
                    self.data[iu] = self.nsolid;
                    self.nsolid += 1;
                } else {
                    // freeze j
                    self.data[ju] = self.nsolid;
                    self.nsolid += 1;
                }
            }

            if self.data[iu] == self.gas {
                self.candidate = i;
            } else if self.data[ju] == self.gas {
                self.candidate = j;
            }

            self.data[iu] < self.data[ju]
        }

        fn swap(&mut self, i: isize, j: isize) {
            self.data.swap(i as usize, j as usize);
        }
    }

    #[test]
    fn adversary() {
        const SIZE: usize = 10_000; // large enough to distinguish between O(n^2) and O(n*log(n))
        let maxcmp = SIZE * lg(SIZE) * 4; // the factor 4 was found by trial and error
        let gas = SIZE as isize - 1;
        let mut d = AdversaryTestingData {
            data: vec![gas; SIZE],
            maxcmp,
            ncmp: 0,
            nsolid: 0,
            candidate: 0,
            gas,
        };
        sort(&mut d, SIZE as isize); // This should degenerate to heapsort.
        // Check data is fully populated and sorted.
        for (i, v) in d.data.iter().enumerate() {
            assert_eq!(*v, i as isize, "adversary data not fully sorted");
        }
    }

    /// `intPairs.inOrder`: whether `a`-equal elements were not reordered.
    fn in_order(d: &[(usize, usize)]) -> bool {
        let (mut last_a, mut last_b) = (None, 0);
        for &(a, b) in d {
            if last_a != Some(a) {
                last_a = Some(a);
                last_b = b;
                continue;
            }
            if b <= last_b {
                return false;
            }
            last_b = b;
        }
        true
    }

    /// `intPairs.initB`: records the initial order in `b`.
    fn init_b(d: &mut [(usize, usize)]) {
        for (i, pair) in d.iter_mut().enumerate() {
            pair.1 = i;
        }
    }

    #[test]
    fn stability() {
        let (n, m) = (100_000usize, 1000usize);
        let less = |x: &(usize, usize), y: &(usize, usize)| x.0 < y.0;
        let mut rng = Rng(3);

        // random distribution
        let mut data: Vec<(usize, usize)> = (0..n).map(|_| (rng.intn(m), 0)).collect();
        assert!(!is_sorted(&data, less), "terrible rand.rand");
        init_b(&mut data);
        slice_stable(&mut data, less);
        assert!(is_sorted(&data, less), "Stable didn't sort {n} ints");
        assert!(in_order(&data), "Stable wasn't stable on {n} ints");

        // already sorted
        init_b(&mut data);
        slice_stable(&mut data, less);
        assert!(
            is_sorted(&data, less),
            "Stable shuffled sorted {n} ints (order)"
        );
        assert!(
            in_order(&data),
            "Stable shuffled sorted {n} ints (stability)"
        );

        // sorted reversed
        for (i, pair) in data.iter_mut().enumerate() {
            pair.0 = n - i;
        }
        init_b(&mut data);
        slice_stable(&mut data, less);
        assert!(is_sorted(&data, less), "Stable didn't sort {n} ints");
        assert!(in_order(&data), "Stable wasn't stable on {n} ints");
    }

    #[test]
    fn short_slices_are_insertion_sorted() {
        // Up to 12 elements pdqsort is an insertion sort, which is stable.
        let mut rng = Rng(11);
        for n in 0..=12 {
            for _ in 0..200 {
                let data: Vec<(usize, usize)> = (0..n).map(|i| (rng.intn(4), i)).collect();
                let mut got = data.clone();
                slice(&mut got, |x, y| x.0 < y.0);
                let mut want = data.clone();
                want.sort_by_key(|p| p.0);
                assert_eq!(got, want, "n={n}");
            }
        }
    }

    #[test]
    fn slice_sorts_every_shape() {
        let mut rng = Rng(5);
        let sizes = (0..=130).chain([200, 255, 256, 257, 500, 1000, 4096]);
        for n in sizes {
            for pattern in 0..6 {
                let data: Vec<(u64, usize)> = (0..n)
                    .map(|i| {
                        let v = match pattern {
                            0 => rng.next_u64() % 3,
                            1 => i as u64,
                            2 => (n - i) as u64,
                            3 => 0,
                            4 => (i % 7) as u64,
                            _ => i.min(n - 1 - i) as u64,
                        };
                        (v, i)
                    })
                    .collect();
                let mut got = data.clone();
                slice(&mut got, |x, y| x.0 > y.0);
                assert!(
                    got.windows(2).all(|w| w[0].0 >= w[1].0),
                    "n={n} pattern={pattern}"
                );
                let mut ids: Vec<usize> = got.iter().map(|p| p.1).collect();
                ids.sort_unstable();
                assert!(
                    ids.iter().enumerate().all(|(i, &id)| i == id),
                    "n={n}: not a permutation"
                );
            }
        }
    }

    #[test]
    fn is_sorted_scans_from_the_end() {
        let v = [1, 3, 2, 4];
        let mut calls = Vec::new();
        assert!(!is_sorted(&v, |a, b| {
            calls.push((*a, *b));
            a < b
        }));
        assert_eq!(calls, [(4, 2), (2, 3)]);
        assert!(is_sorted::<i32>(&[], |a, b| a < b));
        assert!(is_sorted(&[1], |a, b| a < b));
    }
}
