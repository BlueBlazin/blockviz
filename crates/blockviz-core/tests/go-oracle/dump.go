// Command dump prints go/parser's syntax tree for Go files in a canonical
// form that tests/go_parser.rs reproduces from blockviz's own parser, so the
// two can be compared line for line.
//
// Usage:
//
//	go run dump.go FILE...    dump each file
//	go run dump.go -stdin     dump each file named on standard input
//
// Each file's record starts with "### PATH", then "ok" and the dump, or
// "error MESSAGE" when go/parser rejects the file. The dump has one line per
// node in ast.Inspect order, indented by depth: the node type, its start and
// end byte offsets, and its attributes (-1 is a missing position). The
// file's comment groups follow under "comments".
//
// Offsets are raw byte offsets (unaffected by //line directives). go/ast
// computes the end of a comment or raw string from its text, from which the
// scanner removed carriage returns; here ends are true source offsets.
//
// Runs with Go 1.18 and later.
package main

import (
	"bufio"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"strings"
)

func main() {
	out := bufio.NewWriterSize(os.Stdout, 1<<20)
	defer out.Flush()
	if len(os.Args) == 2 && os.Args[1] == "-stdin" {
		in := bufio.NewScanner(os.Stdin)
		in.Buffer(make([]byte, 1<<16), 1<<20)
		for in.Scan() {
			if path := strings.TrimSpace(in.Text()); path != "" {
				dumpFile(out, path)
			}
		}
		return
	}
	for _, path := range os.Args[1:] {
		dumpFile(out, path)
	}
}

func dumpFile(out *bufio.Writer, path string) {
	fmt.Fprintf(out, "### %s\n", path)
	src, err := os.ReadFile(path)
	if err != nil {
		fmt.Fprintf(out, "error %s\n", oneLine(err.Error()))
		return
	}
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, path, src, parser.ParseComments|parser.SkipObjectResolution)
	if err != nil {
		fmt.Fprintf(out, "error %s\n", oneLine(err.Error()))
		return
	}
	out.WriteString("ok\n")
	d := &dumper{out: out, src: src, base: fset.File(file.Package).Base(), fix: map[int]int{}}
	d.trueEnds(file)
	depth := 0
	ast.Inspect(file, func(n ast.Node) bool {
		if n == nil {
			depth--
			return true
		}
		d.node(depth, n)
		depth++
		return true
	})
	out.WriteString("comments\n")
	for _, g := range file.Comments {
		d.node(1, g)
		for _, c := range g.List {
			d.node(2, c)
		}
	}
}

func oneLine(s string) string {
	return strings.ReplaceAll(s, "\n", " ")
}

type dumper struct {
	out  *bufio.Writer
	src  []byte
	base int
	// go/ast's end offset of a comment or raw string that lost carriage
	// returns, mapped to its true end. No other node can end inside one, so
	// any node ending there ends with it.
	fix map[int]int
}

func (d *dumper) trueEnds(file *ast.File) {
	note := func(goEnd, trueEnd int) {
		if goEnd != trueEnd {
			d.fix[goEnd] = trueEnd
		}
	}
	for _, g := range file.Comments {
		for _, c := range g.List {
			start := d.off(c.Slash)
			end := len(d.src)
			if c.Text[1] == '/' {
				if i := strings.IndexByte(string(d.src[start:]), '\n'); i >= 0 {
					end = start + i
				}
				if end-1 >= start+2 && d.src[end-1] == '\r' {
					end--
				}
			} else if i := strings.Index(string(d.src[start+2:]), "*/"); i >= 0 {
				end = start + 2 + i + 2
			}
			note(start+len(c.Text), end)
		}
	}
	ast.Inspect(file, func(n ast.Node) bool {
		if lit, ok := n.(*ast.BasicLit); ok && strings.HasPrefix(lit.Value, "`") {
			start := d.off(lit.ValuePos)
			end := len(d.src)
			if i := strings.IndexByte(string(d.src[start+1:]), '`'); i >= 0 {
				end = start + 1 + i + 1
			}
			note(start+len(lit.Value), end)
		}
		return true
	})
}

func (d *dumper) off(p token.Pos) int {
	if !p.IsValid() {
		return -1
	}
	return int(p) - d.base
}

func (d *dumper) end(n ast.Node) int {
	e := d.off(n.End())
	if t, ok := d.fix[e]; ok {
		return t
	}
	return e
}

func yes(b bool) int {
	if b {
		return 1
	}
	return 0
}

func escape(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '\\':
			b.WriteString(`\\`)
		case c == '\n':
			b.WriteString(`\n`)
		case c == '\r':
			b.WriteString(`\r`)
		case c == '\t':
			b.WriteString(`\t`)
		case c < 0x20 || c == 0x7f:
			fmt.Fprintf(&b, `\x%02x`, c)
		default:
			b.WriteByte(c)
		}
	}
	return b.String()
}

func dir(d ast.ChanDir) string {
	switch d {
	case ast.SEND:
		return "send"
	case ast.RECV:
		return "recv"
	}
	return "both"
}

func (d *dumper) node(depth int, n ast.Node) {
	var attrs string
	name := strings.TrimPrefix(fmt.Sprintf("%T", n), "*ast.")
	switch n := n.(type) {
	case *ast.Comment:
		attrs = " " + escape(n.Text)
	case *ast.Field:
		attrs = fmt.Sprintf(" names=%d tag=%d doc=%d comment=%d", len(n.Names), yes(n.Tag != nil), yes(n.Doc != nil), yes(n.Comment != nil))
	case *ast.FieldList:
		attrs = fmt.Sprintf(" open=%d close=%d", d.off(n.Opening), d.off(n.Closing))
	case *ast.Ident:
		attrs = " " + n.Name
	case *ast.Ellipsis:
		attrs = fmt.Sprintf(" elt=%d", yes(n.Elt != nil))
	case *ast.BasicLit:
		attrs = fmt.Sprintf(" %s %s", n.Kind, escape(n.Value))
	case *ast.CompositeLit:
		attrs = fmt.Sprintf(" type=%d lbrace=%d rbrace=%d elts=%d", yes(n.Type != nil), d.off(n.Lbrace), d.off(n.Rbrace), len(n.Elts))
	case *ast.ParenExpr:
		attrs = fmt.Sprintf(" lparen=%d rparen=%d", d.off(n.Lparen), d.off(n.Rparen))
	case *ast.IndexExpr:
		attrs = fmt.Sprintf(" lbrack=%d rbrack=%d", d.off(n.Lbrack), d.off(n.Rbrack))
	case *ast.IndexListExpr:
		attrs = fmt.Sprintf(" lbrack=%d rbrack=%d indices=%d", d.off(n.Lbrack), d.off(n.Rbrack), len(n.Indices))
	case *ast.SliceExpr:
		attrs = fmt.Sprintf(" lbrack=%d rbrack=%d low=%d high=%d max=%d slice3=%d", d.off(n.Lbrack), d.off(n.Rbrack), yes(n.Low != nil), yes(n.High != nil), yes(n.Max != nil), yes(n.Slice3))
	case *ast.TypeAssertExpr:
		attrs = fmt.Sprintf(" lparen=%d rparen=%d type=%d", d.off(n.Lparen), d.off(n.Rparen), yes(n.Type != nil))
	case *ast.CallExpr:
		attrs = fmt.Sprintf(" lparen=%d ellipsis=%d rparen=%d args=%d", d.off(n.Lparen), d.off(n.Ellipsis), d.off(n.Rparen), len(n.Args))
	case *ast.UnaryExpr:
		attrs = " " + n.Op.String()
	case *ast.BinaryExpr:
		attrs = fmt.Sprintf(" %s op=%d", n.Op, d.off(n.OpPos))
	case *ast.KeyValueExpr:
		attrs = fmt.Sprintf(" colon=%d", d.off(n.Colon))
	case *ast.ArrayType:
		attrs = fmt.Sprintf(" len=%d", yes(n.Len != nil))
	case *ast.FuncType:
		attrs = fmt.Sprintf(" func=%d tparams=%d results=%d", d.off(n.Func), yes(n.TypeParams != nil), yes(n.Results != nil))
	case *ast.ChanType:
		attrs = fmt.Sprintf(" %s arrow=%d", dir(n.Dir), d.off(n.Arrow))
	case *ast.EmptyStmt:
		attrs = fmt.Sprintf(" implicit=%d", yes(n.Implicit))
	case *ast.LabeledStmt:
		attrs = fmt.Sprintf(" colon=%d", d.off(n.Colon))
	case *ast.SendStmt:
		attrs = fmt.Sprintf(" arrow=%d", d.off(n.Arrow))
	case *ast.IncDecStmt:
		attrs = fmt.Sprintf(" %s tok=%d", n.Tok, d.off(n.TokPos))
	case *ast.AssignStmt:
		attrs = fmt.Sprintf(" %s tok=%d lhs=%d rhs=%d", n.Tok, d.off(n.TokPos), len(n.Lhs), len(n.Rhs))
	case *ast.ReturnStmt:
		attrs = fmt.Sprintf(" results=%d", len(n.Results))
	case *ast.BranchStmt:
		attrs = fmt.Sprintf(" %s label=%d", n.Tok, yes(n.Label != nil))
	case *ast.BlockStmt:
		attrs = fmt.Sprintf(" rbrace=%d", d.off(n.Rbrace))
	case *ast.IfStmt:
		attrs = fmt.Sprintf(" init=%d else=%d", yes(n.Init != nil), yes(n.Else != nil))
	case *ast.CaseClause:
		list := fmt.Sprintf("list=%d", len(n.List))
		if n.List == nil {
			list = "default"
		}
		attrs = fmt.Sprintf(" %s colon=%d", list, d.off(n.Colon))
	case *ast.SwitchStmt:
		attrs = fmt.Sprintf(" init=%d tag=%d", yes(n.Init != nil), yes(n.Tag != nil))
	case *ast.TypeSwitchStmt:
		attrs = fmt.Sprintf(" init=%d", yes(n.Init != nil))
	case *ast.CommClause:
		attrs = fmt.Sprintf(" comm=%d colon=%d", yes(n.Comm != nil), d.off(n.Colon))
	case *ast.ForStmt:
		attrs = fmt.Sprintf(" init=%d cond=%d post=%d", yes(n.Init != nil), yes(n.Cond != nil), yes(n.Post != nil))
	case *ast.RangeStmt:
		tok := "none"
		if n.Tok != token.ILLEGAL {
			tok = n.Tok.String()
		}
		attrs = fmt.Sprintf(" key=%d value=%d %s tok=%d", yes(n.Key != nil), yes(n.Value != nil), tok, d.off(n.TokPos))
	case *ast.ImportSpec:
		attrs = fmt.Sprintf(" name=%d doc=%d comment=%d", yes(n.Name != nil), yes(n.Doc != nil), yes(n.Comment != nil))
	case *ast.ValueSpec:
		attrs = fmt.Sprintf(" names=%d type=%d values=%d doc=%d comment=%d", len(n.Names), yes(n.Type != nil), len(n.Values), yes(n.Doc != nil), yes(n.Comment != nil))
	case *ast.TypeSpec:
		attrs = fmt.Sprintf(" tparams=%d assign=%d doc=%d comment=%d", yes(n.TypeParams != nil), d.off(n.Assign), yes(n.Doc != nil), yes(n.Comment != nil))
	case *ast.GenDecl:
		attrs = fmt.Sprintf(" %s lparen=%d rparen=%d specs=%d doc=%d", n.Tok, d.off(n.Lparen), d.off(n.Rparen), len(n.Specs), yes(n.Doc != nil))
	case *ast.FuncDecl:
		attrs = fmt.Sprintf(" recv=%d body=%d doc=%d", yes(n.Recv != nil), yes(n.Body != nil), yes(n.Doc != nil))
	case *ast.File:
		attrs = fmt.Sprintf(" package=%d doc=%d", d.off(n.Package), yes(n.Doc != nil))
	}
	fmt.Fprintf(d.out, "%s%s %d %d%s\n", strings.Repeat("  ", depth), name, d.off(n.Pos()), d.end(n), attrs)
}
