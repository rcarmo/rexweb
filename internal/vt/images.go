package vt

import (
	"bytes"
	"context"
	"image"
	"image/draw"
	"image/png"

	"github.com/tetratelabs/wazero/api"
)

// Decode inside the bounded server VT host without exposing files or network.
func (s *State) decodePNG(ctx context.Context, _ api.Module, _ uint32, allocator, data, length, out uint32) uint32 {
	if length > 8<<20 {
		return 0
	}
	raw, ok := s.module.Memory().Read(data, length)
	if !ok {
		return 0
	}
	input := append([]byte(nil), raw...)
	config, err := png.DecodeConfig(bytes.NewReader(input))
	if err != nil || config.Width <= 0 || config.Height <= 0 || config.Width > 4096 || config.Height > 4096 || int64(config.Width)*int64(config.Height) > 4*1024*1024 {
		return 0
	}
	decoded, err := png.Decode(bytes.NewReader(input))
	if err != nil {
		return 0
	}
	rgba := image.NewNRGBA(decoded.Bounds())
	draw.Draw(rgba, rgba.Bounds(), decoded, decoded.Bounds().Min, draw.Src)
	pointer, err := s.call(ctx, "ghostty_alloc", uint64(allocator), uint64(len(rgba.Pix)))
	if err != nil || pointer == 0 {
		return 0
	}
	if !s.module.Memory().Write(uint32(pointer), rgba.Pix) {
		s.call(ctx, "ghostty_free", uint64(allocator), pointer, uint64(len(rgba.Pix)))
		return 0
	}
	s.put32(s.member(out, "GhosttySysImage", "width"), uint32(config.Width))
	s.put32(s.member(out, "GhosttySysImage", "height"), uint32(config.Height))
	s.put32(s.member(out, "GhosttySysImage", "data"), uint32(pointer))
	s.put32(s.member(out, "GhosttySysImage", "data_len"), uint32(len(rgba.Pix)))
	return 1
}
