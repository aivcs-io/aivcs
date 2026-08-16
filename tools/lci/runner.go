package main

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"strings"
	"time"
)

const defaultTimeout = 30 * time.Minute

// runResult captures the outcome of a single command execution.
type runResult struct {
	ExitCode int
	Output   string
	Duration time.Duration
	Err      error
}

// runCommand executes a command with streaming output, timeout, and
// optional environment overrides.
func runCommand(root string, env map[string]string, verbose bool, name string, args ...string) runResult {
	ctx, cancel := context.WithTimeout(context.Background(), defaultTimeout)
	defer cancel()

	cmd := exec.CommandContext(ctx, name, args...)
	cmd.Dir = root

	// Merge environment
	cmd.Env = os.Environ()
	for k, v := range env {
		cmd.Env = append(cmd.Env, fmt.Sprintf("%s=%s", k, v))
	}

	var buf bytes.Buffer

	if verbose {
		// Stream to terminal AND capture
		cmd.Stdout = io.MultiWriter(os.Stdout, &buf)
		cmd.Stderr = io.MultiWriter(os.Stderr, &buf)
	} else {
		// Capture only, show on failure
		cmd.Stdout = &buf
		cmd.Stderr = &buf
	}

	start := time.Now()
	err := cmd.Run()
	elapsed := time.Since(start)

	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			exitCode = -1
		}
	}

	return runResult{
		ExitCode: exitCode,
		Output:   buf.String(),
		Duration: elapsed,
		Err:      err,
	}
}

// detectSccache returns the path to sccache if it's installed, or "".
func detectSccache() string {
	path, err := exec.LookPath("sccache")
	if err != nil {
		return ""
	}
	return path
}

// cargoCommand builds a cargo invocation with common flags.
func cargoCommand(subcommand string, cfg *Config, extraArgs ...string) (string, []string) {
	args := []string{subcommand}
	args = append(args, extraArgs...)

	if cfg.Jobs > 0 {
		args = append(args, fmt.Sprintf("-j%d", cfg.Jobs))
	}

	return "cargo", args
}

// cargoEnv returns environment variables to set for cargo invocations.
func cargoEnv(cfg *Config) map[string]string {
	env := make(map[string]string)

	// Apply configured env vars, skipping path values that don't exist on
	// this platform (e.g. Linux paths on macOS or vice-versa).
	for k, v := range cfg.EnvVars {
		if strings.HasPrefix(v, "/") {
			if _, err := os.Stat(v); err != nil {
				continue
			}
		}
		env[k] = v
	}

	// Auto-detect PROTOC if not already set via config
	if _, ok := env["PROTOC"]; !ok {
		if p := findBinary("protoc"); p != "" {
			env["PROTOC"] = p
		}
	}

	// Auto-detect LIBCLANG_PATH if not already set via config
	if _, ok := env["LIBCLANG_PATH"]; !ok {
		if p := findLibclang(); p != "" {
			env["LIBCLANG_PATH"] = p
		}
	}

	// Enable sccache if available
	if cfg.SccachePath != "" {
		env["RUSTC_WRAPPER"] = cfg.SccachePath
	}

	// Enable colored output
	env["CARGO_TERM_COLOR"] = "always"

	return env
}

// findBinary locates a binary on PATH using exec.LookPath.
func findBinary(name string) string {
	path, err := exec.LookPath(name)
	if err != nil {
		return ""
	}
	return path
}

// findLibclang attempts to locate libclang's parent directory on common paths.
func findLibclang() string {
	candidates := []string{
		// macOS (Homebrew)
		"/opt/homebrew/opt/llvm/lib",
		"/usr/local/opt/llvm/lib",
		// Linux
		"/usr/lib/llvm-18/lib",
		"/usr/lib/llvm-17/lib",
		"/usr/lib/llvm-16/lib",
		"/usr/lib/llvm-15/lib",
		"/usr/lib/llvm-14/lib",
		"/usr/lib/x86_64-linux-gnu",
		"/usr/lib64",
	}
	for _, dir := range candidates {
		dylib := fmt.Sprintf("%s/libclang.dylib", dir)
		so := fmt.Sprintf("%s/libclang.so", dir)
		if fileExists(dylib) || fileExists(so) {
			return dir
		}
	}
	return ""
}

// fileExists returns true if path exists and is not a directory.
func fileExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}

// buildExcludeArgs converts a list of crate names into --exclude flags.
func buildExcludeArgs(excludes []string) []string {
	var args []string
	for _, e := range excludes {
		args = append(args, "--exclude", e)
	}
	return args
}

// formatCommand returns a human-readable string of the command for display.
func formatCommand(name string, args []string) string {
	parts := append([]string{name}, args...)
	return strings.Join(parts, " ")
}
