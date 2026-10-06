// Package decision is the canonical form of an approval decision (§37 §7.3,
// AP0 Ruling 7). The Android and iOS cores produce the same bytes; the golden
// cases are in conformance/fixtures/decision/claims.json.
package decision

import (
	"fmt"
	"strconv"
	"strings"
)

// Claims are what a decision proof signs.
type Claims struct {
	ApprovalID string
	Revision   uint64
	Decision   string // "approve" or "reject"
	Iat        int64
	Jti        string
}

// Canonical returns the claims as JSON with keys in byte order, no
// whitespace, integers in decimal and strings escaped as RFC 8785 does.
func (c Claims) Canonical() []byte {
	var b strings.Builder
	b.WriteString(`{"approval_id":`)
	b.WriteString(Quote(c.ApprovalID))
	b.WriteString(`,"decision":`)
	b.WriteString(Quote(c.Decision))
	b.WriteString(`,"iat":`)
	b.WriteString(strconv.FormatInt(c.Iat, 10))
	b.WriteString(`,"jti":`)
	b.WriteString(Quote(c.Jti))
	b.WriteString(`,"revision":`)
	b.WriteString(strconv.FormatUint(c.Revision, 10))
	b.WriteString(`}`)
	return []byte(b.String())
}

// Quote escapes a string as RFC 8785 (JCS) does: `"` and `\` escaped,
// \b \f \n \r \t short forms, other controls as \u00xx, everything else raw UTF-8.
func Quote(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if r < 0x20 {
				fmt.Fprintf(&b, `\u%04x`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}
