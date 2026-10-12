package unicode

// Identifiers are Unicode letters and digits.
type Größe struct {
	Breite, Höhe float64
	名前         string
	αβγ         []int
}

var 日本語 = "日本語"

var π = 3.14159

var _ü9 = 'ü'

var ǅ, ǈ int // title-case letters

var x۱ = 1 // an Arabic-Indic digit after a letter

func (g Größe) Fläche() float64 { return g.Breite * g.Höhe }

func émoji() string {
	return "😀 in a string" + `and 🎉 in a raw one`
}

/* Комментарий на русском */
const привет = "мир" // 世界

var r = '世'
