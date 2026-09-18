//! go-udiff v0.4.1 `lcs`: `old.go` (`diff`, `compute`, `editGraph`, `toDiffs`, `forward`,
//! `forwardlcs`, `lookForward`, `setForward`/`getForward`, `backwardlcs`, `lookBackward`,
//! `setBackward`/`getBackward`, `twosided`, `twoDone`, `twolcs`), `common.go`, `labels.go` and
//! `sequence.go`. Go's `backward` is used only by tests and exists here only under `cfg(test)`.
//!
//! Ported line by line, including the search limit of 50 and `fix`'s use of Go's unstable
//! `sort.Slice` ([`crate::gosort::slice`]): both decide the hunks of heavy edits.

use crate::gosort;

/// `lcs.Diff`: a replacement of `a[start..end]` by `b[repl_start..repl_end]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diff {
    pub start: usize,
    pub end: usize,
    pub repl_start: usize,
    pub repl_end: usize,
}

/// `lcs.DiffLines`: the differences between two line sequences.
pub fn diff_lines(a: &[&[u8]], b: &[&[u8]]) -> Vec<Diff> {
    diff(&SliceSeqs { a, b })
}

/// `maxDiffs`: a limit on how deeply the LCS algorithm should search. The value is just a guess.
const MAX_DIFFS: isize = 100;

/// `diff`.
fn diff<S: Sequences>(seqs: &S) -> Vec<Diff> {
    let (diff, _) = compute(seqs, twosided, MAX_DIFFS / 2);
    diff
}

/// `compute` computes the list of differences between two sequences, along with the LCS.
fn compute<S: Sequences>(
    seqs: &S,
    algo: fn(&mut EditGraph<'_, S>) -> Lcs,
    limit: isize,
) -> (Vec<Diff>, Lcs) {
    let limit = if limit <= 0 {
        1 << 25 // effectively infinity
    } else {
        limit
    };
    let (alen, blen) = seqs.lengths();
    let mut g = EditGraph {
        seqs,
        vf: Label::newtriang(limit),
        vb: Label::newtriang(limit),
        limit,
        lx: 0,
        ly: 0,
        ux: alen,
        uy: blen,
        delta: alen - blen,
    };
    let lcs = algo(&mut g);
    let diffs = to_diffs(&lcs, alen, blen);
    (diffs, lcs)
}

/// `editGraph` carries the information for computing the lcs of two sequences.
struct EditGraph<'a, S> {
    seqs: &'a S,
    vf: Label, // forward labels
    vb: Label, // backward labels

    limit: isize, // maximal value of D
    // the bounding rectangle of the current edit graph
    lx: isize,
    ly: isize,
    ux: isize,
    uy: isize,
    delta: isize, // common subexpression: (ux-lx)-(uy-ly); `twolcs` does not update it
}

/// `lcs.toDiffs` converts an LCS to a list of edits.
fn to_diffs(lcs: &[Diag], alen: isize, blen: isize) -> Vec<Diff> {
    let mut diffs = Vec::new();
    let (mut pa, mut pb) = (0, 0); // offsets in a, b
    for l in lcs {
        if pa < l.x || pb < l.y {
            diffs.push(new_diff(pa, l.x, pb, l.y));
        }
        pa = l.x + l.len;
        pb = l.y + l.len;
    }
    if pa < alen || pb < blen {
        diffs.push(new_diff(pa, alen, pb, blen));
    }
    diffs
}

fn new_diff(start: isize, end: isize, repl_start: isize, repl_end: isize) -> Diff {
    Diff {
        start: start as usize,
        end: end as usize,
        repl_start: repl_start as usize,
        repl_end: repl_end as usize,
    }
}

impl<S: Sequences> EditGraph<'_, S> {
    // --- FORWARD ---

    /// `fdone` decides if the forward path has reached the upper right corner of the rectangle. If
    /// so, it also returns the computed lcs.
    fn fdone(&self, d: isize, k: isize) -> Option<Lcs> {
        // x, y, k are relative to the rectangle
        let x = self.vf.get(d, k);
        let y = x - k;
        if x == self.ux && y == self.uy {
            return Some(self.forwardlcs(d, k));
        }
        None
    }

    /// `forwardlcs` recovers the lcs by backtracking from the farthest point reached.
    fn forwardlcs(&self, mut d: isize, mut k: isize) -> Lcs {
        let mut ans = Lcs::new();
        let mut x = self.get_forward(d, k);
        while x != 0 || x - k != 0 {
            if ok(d - 1, k - 1) && x - 1 == self.get_forward(d - 1, k - 1) {
                // if (x-1,y) is labelled D-1, x--,D--,k--,continue
                (d, k, x) = (d - 1, k - 1, x - 1);
                continue;
            } else if ok(d - 1, k + 1) && x == self.get_forward(d - 1, k + 1) {
                // if (x,y-1) is labelled D-1, x, D--,k++, continue
                (d, k) = (d - 1, k + 1);
                continue;
            }
            // if (x-1,y-1)--(x,y) is a diagonal, prepend,x--,y--, continue
            let y = x - k;
            prepend(&mut ans, x + self.lx - 1, y + self.ly - 1);
            x -= 1;
        }
        ans
    }

    /// `lookForward`: start at (x,y), go up the diagonal as far as possible, and label the result.
    fn look_forward(&self, k: isize, relx: isize) -> isize {
        let rely = relx - k;
        let (mut x, y) = (relx + self.lx, rely + self.ly);
        if x < self.ux && y < self.uy {
            x += self.seqs.common_prefix_len(x, self.ux, y, self.uy);
        }
        x
    }

    /// `setForward`.
    fn set_forward(&mut self, d: isize, k: isize, relx: isize) {
        let x = self.look_forward(k, relx);
        self.vf.set(d, k, x - self.lx);
    }

    /// `getForward`.
    fn get_forward(&self, d: isize, k: isize) -> isize {
        self.vf.get(d, k)
    }

    // --- BACKWARD ---

    /// `bdone` decides if the backward path has reached the lower left corner.
    #[cfg(test)]
    fn bdone(&self, d: isize, k: isize) -> Option<Lcs> {
        // x, y, k are relative to the rectangle
        let x = self.vb.get(d, k);
        let y = x - (k + self.delta);
        if x == 0 && y == 0 {
            return Some(self.backwardlcs(d, k));
        }
        None
    }

    /// `backwardlcs` recovers the lcs by backtracking.
    fn backwardlcs(&self, mut d: isize, mut k: isize) -> Lcs {
        let mut ans = Lcs::new();
        let mut x = self.get_backward(d, k);
        while x != self.ux || x - (k + self.delta) != self.uy {
            if ok(d - 1, k - 1) && x == self.get_backward(d - 1, k - 1) {
                // D--, k--, x unchanged
                (d, k) = (d - 1, k - 1);
                continue;
            } else if ok(d - 1, k + 1) && x + 1 == self.get_backward(d - 1, k + 1) {
                // D--, k++, x++
                (d, k, x) = (d - 1, k + 1, x + 1);
                continue;
            }
            let y = x - (k + self.delta);
            append(&mut ans, x + self.lx, y + self.ly);
            x += 1;
        }
        ans
    }

    /// `lookBackward`: start at (x,y), go down the diagonal as far as possible.
    fn look_backward(&self, k: isize, relx: isize) -> isize {
        let rely = relx - (k + self.delta); // forward k = k + e.delta
        let (mut x, y) = (relx + self.lx, rely + self.ly);
        if x > 0 && y > 0 {
            x -= self.seqs.common_suffix_len(0, x, 0, y);
        }
        x
    }

    /// `setBackward`: convert to rectangle, and label the result with d.
    fn set_backward(&mut self, d: isize, k: isize, relx: isize) {
        let x = self.look_backward(k, relx);
        self.vb.set(d, k, x - self.lx);
    }

    /// `getBackward`.
    fn get_backward(&self, d: isize, k: isize) -> isize {
        self.vb.get(d, k)
    }

    // -- TWOSIDED ---

    /// `twoDone`: does Myers' Lemma apply?
    fn two_done(&self, df: isize, db: isize) -> Option<isize> {
        if (df + db + self.delta) % 2 != 0 {
            return None; // diagonals cannot overlap
        }
        let kmin = (-df).max(-db + self.delta);
        let kmax = df.min(db + self.delta);
        let mut k = kmin;
        while k <= kmax {
            let x = self.vf.get(df, k);
            let u = self.vb.get(db, k - self.delta);
            if u <= x {
                // is it worth looking at all the other k?
                let mut l = k;
                while l <= kmax {
                    let x = self.vf.get(df, l);
                    let y = x - l;
                    let u = self.vb.get(db, l - self.delta);
                    let v = u - l;
                    if x == u || u == 0 || v == 0 || y == self.uy || x == self.ux {
                        return Some(l);
                    }
                    l += 2;
                }
                return Some(k);
            }
            k += 2;
        }
        None
    }

    /// `twolcs`.
    fn twolcs(&mut self, df: isize, db: isize, kf: isize) -> Lcs {
        // db==df || db+1==df
        let x = self.vf.get(df, kf);
        let y = x - kf;
        let kb = kf - self.delta;
        let u = self.vb.get(db, kb);
        let v = u - kf;

        // Myers proved there is a df-path from (0,0) to (u,v)
        // and a db-path from (x,y) to (N,M).
        // In the first case the overall path is the forward path
        // to (u,v) followed by the backward path to (N,M).
        // In the second case the path is the backward path to (x,y)
        // followed by the forward path to (x,y) from (0,0).

        // Look for some special cases to avoid computing either of these paths.
        if x == u {
            // "babaab" "cccaba"
            // already patched together
            let mut lcs = self.forwardlcs(df, kf);
            lcs.extend(self.backwardlcs(db, kb));
            return sort(lcs);
        }

        // is (u-1,v) or (u,v-1) labelled df-1?
        // if so, that forward df-1-path plus a horizontal or vertical edge
        // is the df-path to (u,v), then plus the db-path to (N,M)
        if u > 0 && ok(df - 1, u - 1 - v) && self.vf.get(df - 1, u - 1 - v) == u - 1 {
            //  "aabbab" "cbcabc"
            let mut lcs = self.forwardlcs(df - 1, u - 1 - v);
            lcs.extend(self.backwardlcs(db, kb));
            return sort(lcs);
        }
        if v > 0 && ok(df - 1, u - (v - 1)) && self.vf.get(df - 1, u - (v - 1)) == u {
            //  "abaabb" "bcacab"
            let mut lcs = self.forwardlcs(df - 1, u - (v - 1));
            lcs.extend(self.backwardlcs(db, kb));
            return sort(lcs);
        }

        // The path can't possibly contribute to the lcs because it
        // is all horizontal or vertical edges
        if u == 0 || v == 0 || x == self.ux || y == self.uy {
            // "abaabb" "abaaaa"
            if u == 0 || v == 0 {
                return self.backwardlcs(db, kb);
            }
            return self.forwardlcs(df, kf);
        }

        // is (x+1,y) or (x,y+1) labelled db-1?
        // (Go: `x+1 <= e.ux` and `y+1 <= e.uy`.)
        if x < self.ux
            && ok(db - 1, x + 1 - y - self.delta)
            && self.vb.get(db - 1, x + 1 - y - self.delta) == x + 1
        {
            // "bababb" "baaabb"
            let mut lcs = self.backwardlcs(db - 1, kb + 1);
            lcs.extend(self.forwardlcs(df, kf));
            return sort(lcs);
        }
        if y < self.uy
            && ok(db - 1, x - (y + 1) - self.delta)
            && self.vb.get(db - 1, x - (y + 1) - self.delta) == x
        {
            // "abbbaa" "cabacc"
            let mut lcs = self.backwardlcs(db - 1, kb - 1);
            lcs.extend(self.forwardlcs(df, kf));
            return sort(lcs);
        }

        // need to compute another path
        // "aabbaa" "aacaba"
        let mut lcs = self.backwardlcs(db, kb);
        let (oldx, oldy) = (self.ux, self.uy);
        self.ux = u;
        self.uy = v;
        lcs.extend(forward(self));
        (self.ux, self.uy) = (oldx, oldy);
        sort(lcs)
    }
}

/// `forward`: run the forward algorithm, until success or up to the limit on D.
fn forward<S: Sequences>(e: &mut EditGraph<'_, S>) -> Lcs {
    e.set_forward(0, 0, e.lx);
    if let Some(ans) = e.fdone(0, 0) {
        return ans;
    }
    // from D to D+1
    for d in 0..e.limit {
        e.set_forward(d + 1, -(d + 1), e.get_forward(d, -d));
        if let Some(ans) = e.fdone(d + 1, -(d + 1)) {
            return ans;
        }
        e.set_forward(d + 1, d + 1, e.get_forward(d, d) + 1);
        if let Some(ans) = e.fdone(d + 1, d + 1) {
            return ans;
        }
        let mut k = -d + 1;
        while k < d {
            // these are tricky and easy to get backwards
            let lookv = e.look_forward(k, e.get_forward(d, k - 1) + 1);
            let lookh = e.look_forward(k, e.get_forward(d, k + 1));
            if lookv > lookh {
                e.set_forward(d + 1, k, lookv);
            } else {
                e.set_forward(d + 1, k, lookh);
            }
            if let Some(ans) = e.fdone(d + 1, k) {
                return ans;
            }
            k += 2;
        }
    }
    // D is too large
    // find the D path with maximal x+y inside the rectangle and
    // use that to compute the found part of the lcs
    let mut kmax = -e.limit - 1;
    let mut diagmax = -1;
    let mut k = -e.limit;
    while k <= e.limit {
        let x = e.get_forward(e.limit, k);
        let y = x - k;
        if x + y > diagmax && x <= e.ux && y <= e.uy {
            (diagmax, kmax) = (x + y, k);
        }
        k += 2;
    }
    e.forwardlcs(e.limit, kmax)
}

/// `backward`: run the backward algorithm, until success or up to the limit on D (used only by tests).
#[cfg(test)]
fn backward<S: Sequences>(e: &mut EditGraph<'_, S>) -> Lcs {
    e.set_backward(0, 0, e.ux);
    if let Some(ans) = e.bdone(0, 0) {
        return ans;
    }
    // from D to D+1
    for d in 0..e.limit {
        e.set_backward(d + 1, -(d + 1), e.get_backward(d, -d) - 1);
        if let Some(ans) = e.bdone(d + 1, -(d + 1)) {
            return ans;
        }
        e.set_backward(d + 1, d + 1, e.get_backward(d, d));
        if let Some(ans) = e.bdone(d + 1, d + 1) {
            return ans;
        }
        let mut k = -d + 1;
        while k < d {
            // these are tricky and easy to get wrong
            let lookv = e.look_backward(k, e.get_backward(d, k - 1));
            let lookh = e.look_backward(k, e.get_backward(d, k + 1) - 1);
            if lookv < lookh {
                e.set_backward(d + 1, k, lookv);
            } else {
                e.set_backward(d + 1, k, lookh);
            }
            if let Some(ans) = e.bdone(d + 1, k) {
                return ans;
            }
            k += 2;
        }
    }

    // D is too large
    // find the D path with minimal x+y inside the rectangle and
    // use that to compute the part of the lcs found
    let mut kmax = -e.limit - 1;
    let mut diagmin = 1 << 25;
    let mut k = -e.limit;
    while k <= e.limit {
        let x = e.get_backward(e.limit, k);
        let y = x - (k + e.delta);
        if x + y < diagmin && x >= 0 && y >= 0 {
            (diagmin, kmax) = (x + y, k);
        }
        k += 2;
    }
    if kmax < -e.limit {
        panic!("no paths when limit={}?", e.limit);
    }
    e.backwardlcs(e.limit, kmax)
}

/// `twosided`.
fn twosided<S: Sequences>(e: &mut EditGraph<'_, S>) -> Lcs {
    // The termination condition could be improved, as either the forward
    // or backward pass could succeed before Myers' Lemma applies.
    // Aside from questions of efficiency (is the extra testing cost-effective)
    // this is more likely to matter when e.limit is reached.
    e.set_forward(0, 0, e.lx);
    e.set_backward(0, 0, e.ux);

    // from D to D+1
    for d in 0..e.limit {
        // just finished a backwards pass, so check
        if let Some(got) = e.two_done(d, d) {
            return e.twolcs(d, d, got);
        }
        // do a forwards pass (D to D+1)
        e.set_forward(d + 1, -(d + 1), e.get_forward(d, -d));
        e.set_forward(d + 1, d + 1, e.get_forward(d, d) + 1);
        let mut k = -d + 1;
        while k < d {
            // these are tricky and easy to get backwards
            let lookv = e.look_forward(k, e.get_forward(d, k - 1) + 1);
            let lookh = e.look_forward(k, e.get_forward(d, k + 1));
            if lookv > lookh {
                e.set_forward(d + 1, k, lookv);
            } else {
                e.set_forward(d + 1, k, lookh);
            }
            k += 2;
        }
        // just did a forward pass, so check
        if let Some(got) = e.two_done(d + 1, d) {
            return e.twolcs(d + 1, d, got);
        }
        // do a backward pass, D to D+1
        e.set_backward(d + 1, -(d + 1), e.get_backward(d, -d) - 1);
        e.set_backward(d + 1, d + 1, e.get_backward(d, d));
        let mut k = -d + 1;
        while k < d {
            // these are tricky and easy to get wrong
            let lookv = e.look_backward(k, e.get_backward(d, k - 1));
            let lookh = e.look_backward(k, e.get_backward(d, k + 1) - 1);
            if lookv < lookh {
                e.set_backward(d + 1, k, lookv);
            } else {
                e.set_backward(d + 1, k, lookh);
            }
            k += 2;
        }
    }

    // D too large. combine a forward and backward partial lcs
    // first, a forward one
    let mut kmax = -e.limit - 1;
    let mut diagmax = -1;
    let mut k = -e.limit;
    while k <= e.limit {
        let x = e.get_forward(e.limit, k);
        let y = x - k;
        if x + y > diagmax && x <= e.ux && y <= e.uy {
            (diagmax, kmax) = (x + y, k);
        }
        k += 2;
    }
    if kmax < -e.limit {
        panic!("no forward paths when limit={}?", e.limit);
    }
    let mut lcs = e.forwardlcs(e.limit, kmax);
    // now a backward one
    // find the D path with minimal x+y inside the rectangle and
    // use that to compute the lcs
    let mut diagmin = 1 << 25; // infinity
    let mut k = -e.limit;
    while k <= e.limit {
        let x = e.get_backward(e.limit, k);
        let y = x - (k + e.delta);
        if x + y < diagmin && x >= 0 && y >= 0 {
            (diagmin, kmax) = (x + y, k);
        }
        k += 2;
    }
    if kmax < -e.limit {
        panic!("no backward paths when limit={}?", e.limit);
    }
    lcs.extend(e.backwardlcs(e.limit, kmax));
    // These may overlap (e.forwardlcs and e.backwardlcs return sorted lcs)
    fix(lcs)
}

// --- common.go ---

/// `lcs`: a longest common sequence.
type Lcs = Vec<Diag>;

/// `diag`: a piece of the edit graph where `A[X+i] == B[Y+i]`, for `0<=i<Len`. All computed
/// diagonals are parts of a longest common subsequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Diag {
    x: isize,
    y: isize,
    len: isize,
}

/// `lcs.sort`: sorts in place, by lowest X, and if tied, inversely by Len (Go's unstable
/// `sort.Slice`).
fn sort(mut l: Lcs) -> Lcs {
    gosort::slice(
        &mut l,
        |a, b| if a.x != b.x { a.x < b.x } else { a.len > b.len },
    );
    l
}

/// `lcs.valid`: the elements of the lcs do not overlap (expects the lcs to be sorted).
#[cfg(test)]
fn valid(l: &[Diag]) -> bool {
    for i in 1..l.len() {
        if l[i - 1].x + l[i - 1].len > l[i].x {
            return false;
        }
        if l[i - 1].y + l[i - 1].len > l[i].y {
            return false;
        }
    }
    true
}

/// `lcs.fix`: repair an overlapping lcs (only called if two-sided stops early).
fn fix(mut l: Lcs) -> Lcs {
    // from the set of diagonals in l, find a maximal non-conflicting set
    // this problem may be NP-complete, but we use a greedy heuristic,
    // which is quadratic, but with a better data structure, could be D log D.
    // independent is not enough: {0,3,1} and {3,0,2} can't both occur in an lcs
    // which has to have monotone x and y
    if l.is_empty() {
        return Lcs::new();
    }
    gosort::slice(&mut l, |a, b| a.len > b.len);
    let mut tmp = Lcs::with_capacity(l.len());
    tmp.push(l[0]);
    for &item in &l[1..] {
        let mut dir = Direction::Empty;
        let mut nxt = item;
        for &exist in &tmp {
            (dir, nxt) = overlap(exist, nxt);
            if dir == Direction::Empty || dir == Direction::Bad {
                break;
            }
        }
        if nxt.len > 0 && dir != Direction::Bad {
            tmp.push(nxt);
        }
    }
    sort(tmp)
}

/// `direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Empty,    // diag is empty (so not in lcs)
    Leftdown, // proposed acceptably to the left and below
    Rightup,  // proposed diag is acceptably to the right and above
    Bad,      // proposed diag is inconsistent with the lcs so far
}

/// `overlap` trims the proposed diag `prop` so it doesn't overlap with the existing diag that has
/// already been added to the lcs.
fn overlap(exist: Diag, mut prop: Diag) -> (Direction, Diag) {
    if prop.x <= exist.x && exist.x < prop.x + prop.len {
        // remove the end of prop where it overlaps with the X end of exist
        let delta = prop.x + prop.len - exist.x;
        prop.len -= delta;
        if prop.len <= 0 {
            return (Direction::Empty, prop);
        }
    }
    if exist.x <= prop.x && prop.x < exist.x + exist.len {
        // remove the beginning of prop where overlaps with exist
        let delta = exist.x + exist.len - prop.x;
        prop.len -= delta;
        if prop.len <= 0 {
            return (Direction::Empty, prop);
        }
        prop.x += delta;
        prop.y += delta;
    }
    if prop.y <= exist.y && exist.y < prop.y + prop.len {
        // remove the end of prop that overlaps (in Y) with exist
        let delta = prop.y + prop.len - exist.y;
        prop.len -= delta;
        if prop.len <= 0 {
            return (Direction::Empty, prop);
        }
    }
    if exist.y <= prop.y && prop.y < exist.y + exist.len {
        // remove the beginning of peop that overlaps with exist
        let delta = exist.y + exist.len - prop.y;
        prop.len -= delta;
        if prop.len <= 0 {
            return (Direction::Empty, prop);
        }
        prop.x += delta; // no test reaches this code
        prop.y += delta;
    }
    if prop.x + prop.len <= exist.x && prop.y + prop.len <= exist.y {
        return (Direction::Leftdown, prop);
    }
    if exist.x + exist.len <= prop.x && exist.y + exist.len <= prop.y {
        return (Direction::Rightup, prop);
    }
    // prop can't be in an lcs that contains exist
    (Direction::Bad, prop)
}

/// `lcs.prepend`: prepend a diagonal (x,y)-(x+1,y+1) segment either to an empty lcs or to its first
/// Diag. prepend is only called to extend diagonals the backward direction.
fn prepend(lcs: &mut Lcs, x: isize, y: isize) {
    if let Some(d) = lcs.first_mut()
        && d.x == x + 1
        && d.y == y + 1
    {
        // extend the diagonal down and to the left
        (d.x, d.y) = (x, y);
        d.len += 1;
        return;
    }
    lcs.insert(0, Diag { x, y, len: 1 });
}

/// `lcs.append`: appends a diagonal, or extends the existing one, by adding the edge
/// (x,y)-(x+1.y+1). append is only called to extend diagonals in the forward direction.
fn append(lcs: &mut Lcs, x: isize, y: isize) {
    if let Some(last) = lcs.last_mut()
        && last.x + last.len == x
        && last.y + last.len == y
    {
        // Expand last element if adjoining.
        last.len += 1;
        return;
    }
    lcs.push(Diag { x, y, len: 1 });
}

/// `ok`: enforce constraint on d, k.
fn ok(d: isize, k: isize) -> bool {
    d >= 0 && -d <= k && k <= d
}

// --- labels.go ---

/// `label`: for each D, `vec[D]` has length D+1, and the label for (D, k) is stored in
/// `vec[D][(D+k)/2]`. An empty row is Go's nil row.
struct Label {
    vec: Vec<Vec<isize>>,
}

impl Label {
    /// `newtriang`.
    fn newtriang(limit: isize) -> Label {
        if limit < 100 {
            // Preallocate if limit is not large.
            return Label {
                vec: vec![Vec::new(); limit as usize],
            };
        }
        Label { vec: Vec::new() }
    }

    /// `(*label).set`.
    fn set(&mut self, d: isize, k: isize, x: isize) {
        let row = d as usize;
        while self.vec.len() <= row {
            self.vec.push(Vec::new());
        }
        if self.vec[row].is_empty() {
            self.vec[row] = vec![0; row + 1];
        }
        self.vec[row][((d + k) / 2) as usize] = x; // known that D+k is even
    }

    /// `(*label).get`.
    fn get(&self, d: isize, k: isize) -> isize {
        self.vec[d as usize][((d + k) / 2) as usize]
    }
}

// --- sequence.go ---

/// `sequences` abstracts a pair of sequences, A and B.
trait Sequences {
    /// `len(A), len(B)`.
    fn lengths(&self) -> (isize, isize);
    /// `len(commonPrefix(A[ai:aj], B[bi:bj]))`.
    fn common_prefix_len(&self, ai: isize, aj: isize, bi: isize, bj: isize) -> isize;
    /// `len(commonSuffix(A[ai:aj], B[bi:bj]))`.
    fn common_suffix_len(&self, ai: isize, aj: isize, bi: isize, bj: isize) -> isize;
}

/// `linesSeqs` (with `T = &[u8]`), and in tests `bytesSeqs`/`runesSeqs`.
struct SliceSeqs<'a, T> {
    a: &'a [T],
    b: &'a [T],
}

impl<T: PartialEq> Sequences for SliceSeqs<'_, T> {
    fn lengths(&self) -> (isize, isize) {
        (self.a.len() as isize, self.b.len() as isize)
    }

    fn common_prefix_len(&self, ai: isize, aj: isize, bi: isize, bj: isize) -> isize {
        common_prefix_len(
            &self.a[ai as usize..aj as usize],
            &self.b[bi as usize..bj as usize],
        )
    }

    fn common_suffix_len(&self, ai: isize, aj: isize, bi: isize, bj: isize) -> isize {
        common_suffix_len(
            &self.a[ai as usize..aj as usize],
            &self.b[bi as usize..bj as usize],
        )
    }
}

/// `commonPrefixLen` returns the length of the common prefix of `a[ai:aj]` and `b[bi:bj]`.
fn common_prefix_len<T: PartialEq>(a: &[T], b: &[T]) -> isize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i as isize
}

/// `commonSuffixLen` returns the length of the common suffix of `a[ai:aj]` and `b[bi:bj]`.
fn common_suffix_len<T: PartialEq>(a: &[T], b: &[T]) -> isize {
    let n = a.len().min(b.len());
    let mut i = 0;
    while i < n && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i as isize
}

#[cfg(test)]
mod tests {
    //! Ports of go-udiff v0.4.1 `lcs/old_test.go` (`TestAlgosOld`, `TestIntOld`, `TestSpecialOld`,
    //! `TestRegressionOld001`-`003`, `TestRandOld`, `TestDiffAPI`) and `lcs/common_test.go`
    //! (`TestLcsFix`). `udiff/lcs.json` locks `diff_lines` against Go.

    use super::*;
    use crate::gosort::testrng::Rng;

    struct Btest {
        a: &'static str,
        b: &'static str,
        lcs: &'static [&'static str],
    }

    const fn bt(a: &'static str, b: &'static str, lcs: &'static [&'static str]) -> Btest {
        Btest { a, b, lcs }
    }

    const BTESTS: &[Btest] = &[
        bt("aaabab", "abaab", &["abab", "aaab"]),
        bt("aabbba", "baaba", &["aaba"]),
        bt("cabbx", "cbabx", &["cabx", "cbbx"]),
        bt("c", "cb", &["c"]),
        bt("aaba", "bbb", &["b"]),
        bt("bbaabb", "b", &["b"]),
        bt("baaabb", "bbaba", &["bbb", "baa", "bab"]),
        bt("baaabb", "abbab", &["abb", "bab", "aab"]),
        bt("baaba", "aaabba", &["aaba"]),
        bt("ca", "cba", &["ca"]),
        bt("ccbcbc", "abba", &["bb"]),
        bt("ccbcbc", "aabba", &["bb"]),
        bt("ccb", "cba", &["cb"]),
        bt("caef", "axe", &["ae"]),
        bt("bbaabb", "baabb", &["baabb"]),
        // Example from Myers:
        bt("abcabba", "cbabac", &["caba", "baba", "cbba"]),
        bt("3456aaa", "aaa", &["aaa"]),
        bt("aaa", "aaa123", &["aaa"]),
        bt("aabaa", "aacaa", &["aaaa"]),
        bt("1a", "a", &["a"]),
        bt("abab", "bb", &["bb"]),
        bt("123", "ab", &[""]),
        bt("a", "b", &[""]),
        bt("abc", "123", &[""]),
        bt("aa", "aa", &["aa"]),
        bt("abcde", "12345", &[""]),
        bt("aaa3456", "aaa", &["aaa"]),
        bt("abcde", "12345a", &["a"]),
        bt("ab", "123", &[""]),
        bt("1a2", "a", &["a"]),
        // for two-sided
        bt("babaab", "cccaba", &["aba"]),
        bt("aabbab", "cbcabc", &["bab"]),
        bt("abaabb", "bcacab", &["baab"]),
        bt("abaabb", "abaaaa", &["abaa"]),
        bt("bababb", "baaabb", &["baabb"]),
        bt("abbbaa", "cabacc", &["aba"]),
        bt("aabbaa", "aacaba", &["aaaa", "aaba"]),
    ];

    /// `compute(stringSeqs{a, b}, algo, lim)`.
    fn run(algo: &str, a: &str, b: &str, lim: isize) -> (Vec<Diff>, Lcs) {
        let seqs = SliceSeqs {
            a: a.as_bytes(),
            b: b.as_bytes(),
        };
        match algo {
            "forward" => compute(&seqs, forward, lim),
            "backward" => compute(&seqs, backward, lim),
            "twosided" => compute(&seqs, twosided, lim),
            other => panic!("unknown algorithm {other}"),
        }
    }

    /// `check`.
    fn check(s: &str, lcs: &[Diag], want: &[&str]) {
        assert!(valid(lcs), "bad lcs {lcs:?}");
        let mut got = Vec::new();
        for dd in lcs {
            got.extend_from_slice(&s.as_bytes()[dd.x as usize..(dd.x + dd.len) as usize]);
        }
        assert!(
            want.iter().any(|w| w.as_bytes() == got.as_slice()),
            "str={s:?} lcs={lcs:?} want={want:?} got={:?}",
            String::from_utf8_lossy(&got)
        );
    }

    /// `checkDiffs`.
    fn check_diffs(before: &[u8], diffs: &[Diff], after: &[u8]) {
        let mut ans = Vec::new();
        let mut sofar = 0; // index of position in before
        for d in diffs {
            if sofar < d.start {
                ans.extend_from_slice(&before[sofar..d.start]);
            }
            ans.extend_from_slice(&after[d.repl_start..d.repl_end]);
            sofar = d.end;
        }
        ans.extend_from_slice(&before[sofar..]);
        assert!(
            ans == after,
            "diff {diffs:?} took {:?} to {:?}, not to {:?}",
            String::from_utf8_lossy(before),
            String::from_utf8_lossy(&ans),
            String::from_utf8_lossy(after)
        );
    }

    /// `lcslen`.
    fn lcslen(l: &[Diag]) -> isize {
        l.iter().map(|d| d.len).sum()
    }

    #[test]
    fn algos_old() {
        for algo in ["forward", "backward", "twosided"] {
            for tx in BTESTS {
                let lim = (tx.a.len() + tx.b.len()) as isize;

                let (diffs, lcs) = run(algo, tx.a, tx.b, lim);
                check(tx.a, &lcs, tx.lcs);
                check_diffs(tx.a.as_bytes(), &diffs, tx.b.as_bytes());

                let (diffs, lcs) = run(algo, tx.b, tx.a, lim);
                check(tx.b, &lcs, tx.lcs);
                check_diffs(tx.b.as_bytes(), &diffs, tx.a.as_bytes());
            }
        }
    }

    #[test]
    fn int_old() {
        // need to avoid any characters in btests
        let (lfill, rfill) = ("AAAAAAAAAAAA", "BBBBBBBBBBBB");
        for tx in BTESTS {
            if tx.a.len() < 2 || tx.b.len() < 2 {
                continue;
            }
            let left = format!("{}{lfill}", tx.a);
            let right = format!("{}{rfill}", tx.b);
            let lim = (tx.a.len() + tx.b.len()) as isize;
            let (diffs, lcs) = run("twosided", &left, &right, lim);
            check(&left, &lcs, tx.lcs);
            check_diffs(left.as_bytes(), &diffs, right.as_bytes());
            let (diffs, lcs) = run("twosided", &right, &left, lim);
            check(&right, &lcs, tx.lcs);
            check_diffs(right.as_bytes(), &diffs, left.as_bytes());

            let left = format!("{lfill}{}", tx.a);
            let right = format!("{rfill}{}", tx.b);
            let (diffs, lcs) = run("twosided", &left, &right, lim);
            check(&left, &lcs, tx.lcs);
            check_diffs(left.as_bytes(), &diffs, right.as_bytes());
            let (diffs, lcs) = run("twosided", &right, &left, lim);
            check(&right, &lcs, tx.lcs);
            check_diffs(right.as_bytes(), &diffs, left.as_bytes());
        }
    }

    #[test]
    fn special_old() {
        // exercises lcs.fix
        let a = "golang.org/x/tools/intern";
        let b = "github.com/google/safehtml/template\"\n\t\"golang.org/x/tools/intern";
        let (diffs, lcs) = run("twosided", a, b, 4);
        assert!(valid(&lcs), "{},{lcs:?}", diffs.len());
    }

    const REGRESSION_001_A: &str = "// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage diff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n";
    const REGRESSION_001_B: &str = "// Copyright 2019 The Go Authors. All rights reserved.\n// Use of this source code is governed by a BSD-style\n// license that can be found in the LICENSE file.\n\npackage diff_test\n\nimport (\n\t\"fmt\"\n\t\"math/rand\"\n\t\"strings\"\n\t\"testing\"\n\n\t\"github.com/google/safehtml/template\"\n\t\"golang.org/x/tools/gopls/internal/lsp/diff\"\n\t\"github.com/aymanbagabas/go-udiff/difftest\"\n\t\"golang.org/x/tools/gopls/internal/span\"\n)\n";

    #[test]
    fn regression_old_001() {
        let (a, b) = (REGRESSION_001_A, REGRESSION_001_B);
        for i in 1..b.len() {
            let (diffs, lcs) = run("twosided", a, b, i as isize); // 14 from gopls
            assert!(valid(&lcs), "{},{lcs:?}", diffs.len());
            check_diffs(a.as_bytes(), &diffs, b.as_bytes());
        }
    }

    #[test]
    fn regression_old_002() {
        let a = "n\"\n)\n";
        let b = "n\"\n\t\"golang.org/x//nnal/stack\"\n)\n";
        for i in 1..=b.len() {
            let (diffs, lcs) = run("twosided", a, b, i as isize);
            assert!(valid(&lcs), "{},{lcs:?}", diffs.len());
            check_diffs(a.as_bytes(), &diffs, b.as_bytes());
        }
    }

    #[test]
    fn regression_old_003() {
        let a = "golang.org/x/hello v1.0.0\nrequire golang.org/x/unused v1";
        let b = "golang.org/x/hello v1";
        for i in 1..=a.len() {
            let (diffs, lcs) = run("twosided", a, b, i as isize);
            assert!(valid(&lcs), "{},{lcs:?}", diffs.len());
            check_diffs(a.as_bytes(), &diffs, b.as_bytes());
        }
    }

    /// `randstr`: a random string of `n` runes from `s`.
    fn randstr(rng: &mut Rng, s: &str, n: usize) -> Vec<char> {
        let src: Vec<char> = s.chars().collect();
        (0..n).map(|_| src[rng.intn(src.len())]).collect()
    }

    #[test]
    fn rand_old() {
        let mut rng = Rng(0x0dd5_eed5);
        for i in 0..1000 {
            let a = randstr(&mut rng, "abω", 16);
            let b = randstr(&mut rng, "abωc", 16);
            let seq = SliceSeqs { a: &a, b: &b };

            const LIM: isize = 0; // make sure we get the lcs (24 was too small)
            let (_, forw) = compute(&seq, forward, LIM);
            let (_, back) = compute(&seq, backward, LIM);
            let (_, two) = compute(&seq, twosided, LIM);
            assert!(
                lcslen(&two) == lcslen(&forw) && lcslen(&forw) == lcslen(&back),
                "{i} forw:{} back:{} two:{}\n{forw:?}\n{back:?}\n{two:?}",
                lcslen(&forw),
                lcslen(&back),
                lcslen(&two)
            );
            assert!(valid(&two) && valid(&forw) && valid(&back), "check failure");
        }
    }

    fn d(x: isize, y: isize, len: isize) -> Diag {
        Diag { x, y, len }
    }

    #[test]
    fn lcs_fix() {
        // (before, after of TestLcsFix, the exact result of Go's fix()).
        let tests = [
            (
                vec![d(0, 0, 3), d(2, 2, 5), d(3, 4, 5), d(8, 9, 4)],
                vec![d(0, 0, 2), d(2, 2, 1), d(3, 4, 5), d(8, 9, 4)],
                vec![d(0, 0, 2), d(2, 2, 5), d(7, 8, 1), d(8, 9, 4)],
            ),
            (
                vec![d(1, 1, 6), d(6, 12, 3)],
                vec![d(1, 1, 5), d(6, 12, 3)],
                vec![d(1, 1, 6), d(7, 13, 2)],
            ),
            (
                vec![d(0, 0, 4), d(3, 5, 4)],
                vec![d(0, 0, 3), d(3, 5, 4)],
                vec![d(0, 0, 4), d(4, 6, 3)],
            ),
            (
                vec![d(0, 20, 1), d(0, 0, 3), d(1, 20, 4)],
                vec![d(0, 0, 3), d(3, 22, 2)],
                vec![d(0, 0, 1), d(1, 20, 4)],
            ),
            (
                vec![d(0, 0, 4), d(1, 1, 2)],
                vec![d(0, 0, 4)],
                vec![d(0, 0, 4)],
            ),
            (vec![d(0, 0, 4)], vec![d(0, 0, 4)], vec![d(0, 0, 4)]),
            (vec![], vec![], vec![]),
            (
                vec![d(0, 0, 4), d(1, 1, 6), d(3, 3, 2)],
                vec![d(0, 0, 1), d(1, 1, 6)],
                vec![d(0, 0, 1), d(1, 1, 6)],
            ),
        ];
        for (n, (before, after, go)) in tests.into_iter().enumerate() {
            let got = fix(before.clone());
            assert_eq!(
                got.len(),
                after.len(),
                "got {got:?}, expected {after:?}, for {before:?}"
            );
            assert_eq!(
                lcslen(&got),
                lcslen(&after),
                "{n}: lens differ, {got:?}, {after:?}, {before:?}"
            );
            assert_eq!(got, go, "{n}: fix({before:?}) differs from Go's");
        }
    }

    #[test]
    fn diff_api() {
        let bytes = |a: &str, b: &str| {
            diff(&SliceSeqs {
                a: a.as_bytes(),
                b: b.as_bytes(),
            })
        };
        let runes = |a: &str, b: &str| {
            let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
            diff(&SliceSeqs { a: &a, b: &b })
        };
        let x = |start, end, repl_start, repl_end| Diff {
            start,
            end,
            repl_start,
            repl_end,
        };
        assert_eq!(bytes("abcXdef", "abcxdef"), [x(3, 4, 3, 4)]); // ASCII
        assert_eq!(runes("abcXdef", "abcxdef"), [x(3, 4, 3, 4)]);
        assert_eq!(bytes("abcωdef", "abcΩdef"), [x(3, 5, 3, 5)]); // non-ASCII
        assert_eq!(runes("abcωdef", "abcΩdef"), [x(3, 4, 3, 4)]);
    }

    #[test]
    fn diff_lines_of_lines() {
        let a: [&[u8]; 3] = [b"a\n", b"b\n", b"c\n"];
        let b: [&[u8]; 3] = [b"a\n", b"B\n", b"c\n"];
        assert_eq!(
            diff_lines(&a, &b),
            [Diff {
                start: 1,
                end: 2,
                repl_start: 1,
                repl_end: 2
            }]
        );
        assert!(diff_lines(&a, &a).is_empty());
        assert!(diff_lines(&[], &[]).is_empty());
        assert_eq!(
            diff_lines(&[], &a),
            [Diff {
                start: 0,
                end: 0,
                repl_start: 0,
                repl_end: 3
            }]
        );
    }
}
