package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Config holds the resolved workspace configuration used by the pipeline.
type Config struct {
	Root          string
	WorkspaceName string
	Members       []string
	Exclude       []string
	TestExclude   []string
	DefaultStages []string
	EnvVars       map[string]string
	UseSccache    bool
	SccachePath   string
	Jobs          int
}

// defaultTestExclude mirrors the exclusions in ci.yml.
// All aivcs crates are tested by default — add entries here if needed.
var defaultTestExclude = []string{}

// findWorkspaceRoot walks up from dir looking for a Cargo.toml that contains
// a [workspace] section.
func findWorkspaceRoot(dir string) (string, error) {
	abs, err := filepath.Abs(dir)
	if err != nil {
		return "", err
	}
	for {
		cargoPath := filepath.Join(abs, "Cargo.toml")
		if data, err := os.ReadFile(cargoPath); err == nil {
			if strings.Contains(string(data), "[workspace]") {
				return abs, nil
			}
		}
		parent := filepath.Dir(abs)
		if parent == abs {
			break
		}
		abs = parent
	}
	return "", fmt.Errorf("no Cargo.toml with [workspace] found above %s", dir)
}

// loadConfig reads the workspace Cargo.toml and optional .lci.toml to
// produce a merged Config.
func loadConfig(root string) (*Config, error) {
	cargoPath := filepath.Join(root, "Cargo.toml")
	members, exclude, err := parseCargoWorkspace(cargoPath)
	if err != nil {
		return nil, fmt.Errorf("parsing %s: %w", cargoPath, err)
	}

	cfg := &Config{
		Root:          root,
		WorkspaceName: filepath.Base(root),
		Members:       members,
		Exclude:       exclude,
		TestExclude:   defaultTestExclude,
		DefaultStages: []string{"fmt", "clippy", "test"},
		EnvVars:       map[string]string{},
		UseSccache:    true,
		Jobs:          0,
	}

	// Override with .lci.toml if present
	lciPath := filepath.Join(root, ".lci.toml")
	if _, err := os.Stat(lciPath); err == nil {
		if err := applyLCIConfig(cfg, lciPath); err != nil {
			return nil, fmt.Errorf("parsing %s: %w", lciPath, err)
		}
	}

	return cfg, nil
}

// parseCargoWorkspace extracts workspace members and exclude lists from a
// Cargo.toml file. It uses a simple line-based parser — no external TOML
// library needed.
func parseCargoWorkspace(path string) (members []string, exclude []string, err error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, nil, err
	}

	lines := strings.Split(string(data), "\n")
	inWorkspace := false
	var target *[]string
	inArray := false

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		// Track section headers
		if strings.HasPrefix(trimmed, "[") && strings.HasSuffix(trimmed, "]") {
			section := trimmed[1 : len(trimmed)-1]
			inWorkspace = section == "workspace"
			inArray = false
			target = nil
			continue
		}
		// Also match [workspace.xxx] subsections as still in workspace
		if strings.HasPrefix(trimmed, "[workspace.") {
			inWorkspace = true
			inArray = false
			target = nil
			continue
		}

		if !inWorkspace {
			continue
		}

		// Detect start of members/exclude arrays
		if !inArray {
			if strings.HasPrefix(trimmed, "members") && strings.Contains(trimmed, "=") {
				target = &members
				if strings.Contains(trimmed, "[") {
					inArray = true
					if strings.Contains(trimmed, "]") {
						// Single-line array
						extractAllQuoted(trimmed, target)
						inArray = false
						target = nil
					}
				}
				continue
			}
			if strings.HasPrefix(trimmed, "exclude") && strings.Contains(trimmed, "=") {
				target = &exclude
				if strings.Contains(trimmed, "[") {
					inArray = true
					if strings.Contains(trimmed, "]") {
						extractAllQuoted(trimmed, target)
						inArray = false
						target = nil
					}
				}
				continue
			}
			continue
		}

		// Collecting array items
		if inArray && target != nil {
			if strings.Contains(trimmed, "]") {
				// Last line of array — may still have a value
				if v := extractFirstQuoted(trimmed); v != "" {
					*target = append(*target, v)
				}
				inArray = false
				target = nil
				continue
			}
			if v := extractFirstQuoted(trimmed); v != "" {
				*target = append(*target, v)
			}
		}
	}

	return members, exclude, nil
}

// applyLCIConfig reads a .lci.toml and overrides Config fields.
// Supported sections: [test], [stages], [env], [performance].
func applyLCIConfig(cfg *Config, path string) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}

	lines := strings.Split(string(data), "\n")
	section := ""
	var target *[]string
	inArray := false

	for _, line := range lines {
		trimmed := strings.TrimSpace(line)

		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}

		if strings.HasPrefix(trimmed, "[") && strings.HasSuffix(trimmed, "]") {
			section = trimmed[1 : len(trimmed)-1]
			inArray = false
			target = nil
			continue
		}

		if inArray && target != nil {
			if strings.Contains(trimmed, "]") {
				if v := extractFirstQuoted(trimmed); v != "" {
					*target = append(*target, v)
				}
				inArray = false
				target = nil
				continue
			}
			if v := extractFirstQuoted(trimmed); v != "" {
				*target = append(*target, v)
			}
			continue
		}

		parts := strings.SplitN(trimmed, "=", 2)
		if len(parts) != 2 {
			continue
		}
		key := strings.TrimSpace(parts[0])
		value := strings.TrimSpace(parts[1])

		switch section {
		case "test":
			if key == "exclude" {
				cfg.TestExclude = []string{}
				target = &cfg.TestExclude
				if strings.Contains(value, "[") {
					inArray = true
					if strings.Contains(value, "]") {
						extractAllQuoted(value, &cfg.TestExclude)
						inArray = false
						target = nil
					}
				}
			}

		case "stages":
			if key == "default" {
				cfg.DefaultStages = []string{}
				target = &cfg.DefaultStages
				if strings.Contains(value, "[") {
					inArray = true
					if strings.Contains(value, "]") {
						extractAllQuoted(value, &cfg.DefaultStages)
						inArray = false
						target = nil
					}
				}
			}

		case "env":
			if v := extractFirstQuoted(value); v != "" {
				cfg.EnvVars[key] = v
			} else {
				cfg.EnvVars[key] = strings.Trim(value, "\"' ")
			}

		case "performance":
			if key == "sccache" {
				cfg.UseSccache = value == "true"
			}
			if key == "jobs" {
				n := 0
				fmt.Sscanf(value, "%d", &n)
				cfg.Jobs = n
			}
		}
	}

	return nil
}

// generateDefaultConfig writes a .lci.toml with sensible defaults for the
// current workspace.
func generateDefaultConfig(root string) error {
	content := `# lci — Local CI configuration for aivcs
# Auto-generated. Edit to customize local CI behavior.

[stages]
# Stages to run by default (in order). Options: fmt, clippy, test, check, build
default = ["fmt", "clippy"]

[test]
# Crates excluded from workspace tests (empty = test all crates)
exclude = []

[env]
# Environment variables set during CI stages.

[performance]
# Use sccache for cached Rust compilation if available
sccache = true
# Parallel cargo jobs (0 = auto-detect)
jobs = 0
`
	return os.WriteFile(filepath.Join(root, ".lci.toml"), []byte(content), 0644)
}

// extractFirstQuoted returns the first double-quoted string in s, or "".
func extractFirstQuoted(s string) string {
	start := strings.Index(s, "\"")
	if start < 0 {
		return ""
	}
	end := strings.Index(s[start+1:], "\"")
	if end < 0 {
		return ""
	}
	return s[start+1 : start+1+end]
}

// extractAllQuoted appends every double-quoted string in s to dst.
func extractAllQuoted(s string, dst *[]string) {
	remaining := s
	for {
		start := strings.Index(remaining, "\"")
		if start < 0 {
			return
		}
		end := strings.Index(remaining[start+1:], "\"")
		if end < 0 {
			return
		}
		*dst = append(*dst, remaining[start+1:start+1+end])
		remaining = remaining[start+1+end+1:]
	}
}

// checkEnv verifies that system dependencies expected by the CI workflow are
// present on the local machine.
func checkEnv(ui *UI, cfg *Config) {
	type dep struct {
		name string
		cmds []string // try each, first success wins
		hint string
	}

	deps := []dep{
		{"cargo", []string{"cargo"}, "Install via https://rustup.rs"},
		{"rustfmt", []string{"rustfmt"}, "rustup component add rustfmt"},
		{"clippy", []string{"cargo-clippy"}, "rustup component add clippy"},
		{"protoc", []string{"protoc"}, "apt install protobuf-compiler"},
	}

	allOk := true
	for _, d := range deps {
		found := false
		for _, cmd := range d.cmds {
			if lookPath(cmd) {
				found = true
				break
			}
		}
		if found {
			ui.envOk(d.name)
		} else {
			ui.envMissing(d.name, d.hint)
			allOk = false
		}
	}

	// Check sccache
	if detectSccache() != "" {
		ui.envOk("sccache")
	} else {
		ui.envOptional("sccache", "cargo install sccache (optional, speeds up compilation)")
	}

	if allOk {
		ui.success("All required dependencies found")
	} else {
		ui.warn("Some dependencies are missing — CI stages may fail")
	}
}

// lookPath checks if a binary is on PATH.
func lookPath(name string) bool {
	for _, dir := range filepath.SplitList(os.Getenv("PATH")) {
		if _, err := os.Stat(filepath.Join(dir, name)); err == nil {
			return true
		}
	}
	return false
}
