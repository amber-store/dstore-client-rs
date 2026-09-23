// Verbatim copy of github.com/amber-store/dstore v0.1.11 cmd/dstore/wc_test.go, run
// against the copies of copied.go (only the package clause differs).

package mainpkg

import "testing"

func TestResolveTicket(t *testing.T) {
	cases := []struct{ flag, stored, env, want string }{
		{"f", "s", "e", "f"},
		{"", "s", "e", "s"},
		{"", "", "e", "e"},
	}
	for _, c := range cases {
		got, err := resolveTicket(c.flag, c.stored, c.env)
		if err != nil || got != c.want {
			t.Errorf("resolveTicket(%q,%q,%q) = %q, %v; want %q", c.flag, c.stored, c.env, got, err, c.want)
		}
	}
	if _, err := resolveTicket("", "", ""); err == nil {
		t.Error("no ticket anywhere must be an error")
	}
}
