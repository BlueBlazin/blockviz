// Package unicode has non-ASCII identifiers and text.
package unicode

// Grüße greets in German.
func Grüße(名前 string) string {
	return "Grüße, " + 名前 + " 👋"
}

// AfterUnicode follows multi-byte text.
func AfterUnicode() string { return "π ≈ 3.14159" }
