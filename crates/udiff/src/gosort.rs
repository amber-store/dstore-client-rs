//! Go 1.26.5 `sort`: `sort/zsortfunc.go` (`pdqsort_func` and helpers), `sort/sort.go:59-80`,
//! `sort/slice.go`, and `sort.Stable`.

/// `sort.Slice` = `pdqsort_func(data, 0, n, bits.Len(n))`.
pub fn slice<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool) {
    todo!()
}

/// `sort.SliceStable` / `sort.Stable` (insertion sort + symMerge).
pub fn slice_stable<T>(v: &mut [T], less: impl FnMut(&T, &T) -> bool) {
    todo!()
}

/// `sort.SliceIsSorted`.
pub fn is_sorted<T>(v: &[T], less: impl FnMut(&T, &T) -> bool) -> bool {
    todo!()
}
