package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestExtractFirstQuoted(t *testing.T) {
	tests := []struct {
		input string
		want  string
	}{
		{`"hello"`, "hello"},
		{`  "crates/foo",`, "crates/foo"},
		{`key = "value"`, "value"},
		{`no quotes here`, ""},
		{`"unclosed`, ""},
		{`""`, ""},
	}
	for _, tt := range tests {
		got := extractFirstQuoted(tt.input)
		if got != tt.want {
			t.Errorf("extractFirstQuoted(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}

func TestExtractAllQuoted(t *testing.T) {
	tests := []struct {
		input string
		want  []string
	}{
		{`["fmt", "clippy", "test"]`, []string{"fmt", "clippy", "test"}},
		{`["one"]`, []string{"one"}},
		{`[]`, nil},
		{`"a", "b"`, []string{"a", "b"}},
	}
	for _, tt := range tests {
		var got []string
		extractAllQuoted(tt.input, &got)
		if len(got) != len(tt.want) {
			t.Errorf("extractAllQuoted(%q): got %v, want %v", tt.input, got, tt.want)
			continue
		}
		for i := range got {
			if got[i] != tt.want[i] {
				t.Errorf("extractAllQuoted(%q)[%d] = %q, want %q", tt.input, i, got[i], tt.want[i])
			}
		}
	}
}

func TestParseCargoWorkspace(t *testing.T) {
	content := `[package]
name = "root"

[workspace]
resolver = "2"

members = [
    "crates/alpha",
    "crates/beta",
    "apps/gamma",
]

exclude = [
    "legacy/old",
]

[workspace.dependencies]
tokio = "1.0"
`
	dir := t.TempDir()
	path := filepath.Join(dir, "Cargo.toml")
	if err := os.WriteFile(path, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	members, exclude, err := parseCargoWorkspace(path)
	if err != nil {
		t.Fatal(err)
	}

	if len(members) != 3 {
		t.Fatalf("expected 3 members, got %d: %v", len(members), members)
	}
	if members[0] != "crates/alpha" || members[1] != "crates/beta" || members[2] != "apps/gamma" {
		t.Errorf("unexpected members: %v", members)
	}
	if len(exclude) != 1 || exclude[0] != "legacy/old" {
		t.Errorf("unexpected exclude: %v", exclude)
	}
}

func TestFindWorkspaceRoot(t *testing.T) {
	dir := t.TempDir()
	// Create nested structure: dir/sub/deep/
	sub := filepath.Join(dir, "sub", "deep")
	os.MkdirAll(sub, 0755)

	// Write Cargo.toml with [workspace] at root
	cargo := filepath.Join(dir, "Cargo.toml")
	os.WriteFile(cargo, []byte("[workspace]\nmembers = []\n"), 0644)

	// Should find workspace from nested dir
	root, err := findWorkspaceRoot(sub)
	if err != nil {
		t.Fatal(err)
	}
	if root != dir {
		t.Errorf("expected root %q, got %q", dir, root)
	}
}

func TestFindWorkspaceRootNotFound(t *testing.T) {
	dir := t.TempDir()
	_, err := findWorkspaceRoot(dir)
	if err == nil {
		t.Error("expected error for directory without Cargo.toml")
	}
}

func TestCache(t *testing.T) {
	dir := t.TempDir()

	// Fresh cache
	c := newCache()
	if _, ok := c.get("fmt"); ok {
		t.Error("expected empty cache")
	}

	// Set and get
	c.set("fmt", "abc123")
	hash, ok := c.get("fmt")
	if !ok || hash != "abc123" {
		t.Errorf("expected abc123, got %q (ok=%v)", hash, ok)
	}

	// Save and reload
	if err := saveCache(c, dir); err != nil {
		t.Fatal(err)
	}

	c2, err := loadCache(dir)
	if err != nil {
		t.Fatal(err)
	}
	hash2, ok := c2.get("fmt")
	if !ok || hash2 != "abc123" {
		t.Errorf("reloaded cache: expected abc123, got %q", hash2)
	}
}

func TestPipelineStages(t *testing.T) {
	cfg := &Config{
		TestExclude: []string{"foo", "bar"},
		DefaultStages: []string{"fmt", "clippy", "test"},
	}

	p := newPipeline(cfg, []string{"fmt", "clippy", "test", "check"}, false)

	if len(p.stages) != 4 {
		t.Fatalf("expected 4 stages, got %d", len(p.stages))
	}

	// fmt stage should have --check
	fmtArgs := p.stages[0].Args
	found := false
	for _, a := range fmtArgs {
		if a == "--check" {
			found = true
		}
	}
	if !found {
		t.Errorf("fmt stage missing --check: %v", fmtArgs)
	}

	// test stage should have --exclude flags
	testArgs := p.stages[2].Args
	excludeCount := 0
	for _, a := range testArgs {
		if a == "--exclude" {
			excludeCount++
		}
	}
	if excludeCount != 2 {
		t.Errorf("expected 2 --exclude in test args, got %d: %v", excludeCount, testArgs)
	}
}

func TestPipelineFix(t *testing.T) {
	cfg := &Config{
		TestExclude:   []string{},
		DefaultStages: []string{"fmt"},
	}

	p := newPipeline(cfg, []string{"fmt"}, true)

	// fmt with --fix should NOT have --check
	for _, a := range p.stages[0].Args {
		if a == "--check" {
			t.Error("fmt stage should not have --check when --fix is enabled")
		}
	}
}

func TestFormatDuration(t *testing.T) {
	tests := []struct {
		dur  time.Duration
		want string
	}{
		{0, ""},
		{500 * time.Millisecond, "500ms"},
		{1500 * time.Millisecond, "1.5s"},
		{65 * time.Second, "1m5.0s"},
	}
	for _, tt := range tests {
		got := formatDuration(tt.dur)
		if got != tt.want {
			t.Errorf("formatDuration(%v) = %q, want %q", tt.dur, got, tt.want)
		}
	}
}

func TestApplyLCIConfig(t *testing.T) {
	content := `[test]
exclude = [
    "alpha",
    "beta",
]

[stages]
default = ["fmt", "test"]

[env]
FOO = "bar"

[performance]
sccache = false
jobs = 4
`
	dir := t.TempDir()
	path := filepath.Join(dir, ".lci.toml")
	os.WriteFile(path, []byte(content), 0644)

	cfg := &Config{
		TestExclude:   []string{"old"},
		DefaultStages: []string{"fmt", "clippy", "test"},
		EnvVars:       map[string]string{},
	}
	if err := applyLCIConfig(cfg, path); err != nil {
		t.Fatal(err)
	}
	if len(cfg.TestExclude) != 2 || cfg.TestExclude[0] != "alpha" {
		t.Errorf("unexpected TestExclude: %v", cfg.TestExclude)
	}
	if len(cfg.DefaultStages) != 2 || cfg.DefaultStages[0] != "fmt" {
		t.Errorf("unexpected DefaultStages: %v", cfg.DefaultStages)
	}
	if cfg.EnvVars["FOO"] != "bar" {
		t.Errorf("unexpected env FOO: %q", cfg.EnvVars["FOO"])
	}
	if cfg.UseSccache != false {
		t.Error("expected sccache=false")
	}
	if cfg.Jobs != 4 {
		t.Errorf("expected jobs=4, got %d", cfg.Jobs)
	}
}

