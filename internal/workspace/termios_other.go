//go:build !darwin && !linux

package workspace

func (p *pane) inputMode() (fields, error) {
	return nil, unsupported("terminal input mode requires macOS")
}
