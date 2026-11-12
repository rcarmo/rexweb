package vt

import (
	"bytes"
	"net/url"
	"strings"
	"unicode"

	"golang.org/x/net/html"
)

const ExportCSP = "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'"

// Render exported HTML as an offline document. Even metadata links are untrusted.
func sanitizeHTML(source []byte) ([]byte, error) {
	doc, err := html.Parse(bytes.NewReader(source))
	if err != nil {
		return nil, err
	}
	var walk func(*html.Node)
	walk = func(node *html.Node) {
		if node.Type == html.ElementNode {
			attrs := node.Attr[:0]
			for _, attribute := range node.Attr {
				if strings.HasPrefix(strings.ToLower(attribute.Key), "on") || attribute.Key == "src" || attribute.Key == "srcdoc" {
					continue
				}
				if attribute.Key == "href" {
					uri, err := url.Parse(attribute.Val)
					if err != nil || strings.IndexFunc(attribute.Val, unicode.IsControl) >= 0 || !safeLinkScheme(uri.Scheme) {
						continue
					}
				}
				attrs = append(attrs, attribute)
			}
			node.Attr = attrs
			if node.Data == "head" {
				meta := &html.Node{Type: html.ElementNode, Data: "meta", Attr: []html.Attribute{{Key: "http-equiv", Val: "Content-Security-Policy"}, {Key: "content", Val: ExportCSP}}}
				node.AppendChild(meta)
			}
		}
		for child := node.FirstChild; child != nil; {
			next := child.NextSibling
			if child.Type == html.ElementNode && (child.Data == "script" || child.Data == "iframe" || child.Data == "object" || child.Data == "embed" || child.Data == "base" || child.Data == "form") {
				node.RemoveChild(child)
			} else {
				walk(child)
			}
			child = next
		}
	}
	walk(doc)
	var result bytes.Buffer
	if err := html.Render(&result, doc); err != nil {
		return nil, err
	}
	return result.Bytes(), nil
}
func safeLinkScheme(scheme string) bool {
	switch strings.ToLower(scheme) {
	case "http", "https", "mailto", "ftp", "ssh", "gemini", "gopher", "tel":
		return true
	}
	return false
}
