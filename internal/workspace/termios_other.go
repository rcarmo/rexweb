//go:build !darwin

package workspace

func (p *pane) inputMode() (fields, error) {
	return nil, unsupported("terminal input mode requires macOS")
}
