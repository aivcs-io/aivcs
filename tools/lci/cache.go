package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"
)

const cacheDir = ".lci"
const cacheFile = "cache.json"

// Cache stores per-stage source hashes so stages can be skipped when the
// workspace source has not changed since the last successful run.
type Cache struct {
	Version int                    `json:"version"`
	Stages  map[string]stageEntry  `json:"stages"`
}

type stageEntry struct {
	Hash string `json:"hash"`
	At   string `json:"at"`
}

func newCache() *Cache {
	return &Cache{
		Version: 1,
		Stages:  make(map[string]stageEntry),
	}
}

func loadCache(root string) (*Cache, error) {
	path := filepath.Join(root, cacheDir, cacheFile)
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return newCache(), nil
		}
		return nil, err
	}

	c := newCache()
	if err := json.Unmarshal(data, c); err != nil {
		// Corrupted cache — start fresh
		return newCache(), nil
	}
	return c, nil
}

func saveCache(c *Cache, root string) error {
	dir := filepath.Join(root, cacheDir)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(filepath.Join(dir, cacheFile), data, 0644)
}

func (c *Cache) get(stage string) (string, bool) {
	e, ok := c.Stages[stage]
	if !ok {
		return "", false
	}
	return e.Hash, true
}

func (c *Cache) set(stage, hash string) {
	c.Stages[stage] = stageEntry{
		Hash: hash,
		At:   time.Now().UTC().Format(time.RFC3339),
	}
}

// computeSourceHash produces a single SHA256 digest representing the current
// state of all Rust source and configuration files.
//
// Strategy (fast path): if we are in a git repo, use `git ls-files` to
// enumerate tracked files, then hash their content via `git hash-object`.
// This is significantly faster than walking the filesystem because git's
// index is already in memory.
//
// Fallback: walk the filesystem for .rs, Cargo.toml, and Cargo.lock files.
func computeSourceHash(root string) (string, error) {
	hash, err := gitSourceHash(root)
	if err == nil && hash != "" {
		return hash, nil
	}
	return filesystemSourceHash(root)
}

// gitSourceHash uses git to compute a fast workspace hash.
// It hashes: git diff output + Cargo.lock content.
// Any uncommitted change (staged or unstaged) changes the hash.
func gitSourceHash(root string) (string, error) {
	// Check we're in a git repo
	cmd := exec.Command("git", "rev-parse", "--is-inside-work-tree")
	cmd.Dir = root
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("not a git repo")
	}

	h := sha256.New()

	// 1. Hash of HEAD (captures committed changes)
	headCmd := exec.Command("git", "rev-parse", "HEAD")
	headCmd.Dir = root
	if out, err := headCmd.Output(); err == nil {
		h.Write(out)
	}

	// 2. Hash of diff against HEAD for Rust-relevant files (captures uncommitted changes)
	diffCmd := exec.Command("git", "diff", "HEAD", "--",
		"*.rs", "Cargo.toml", "Cargo.lock", ".lci.toml")
	diffCmd.Dir = root
	if out, err := diffCmd.Output(); err == nil {
		h.Write(out)
	}

	// 3. Hash of untracked .rs and Cargo.toml files
	untrackedCmd := exec.Command("git", "ls-files", "--others", "--exclude-standard",
		"*.rs", "Cargo.toml")
	untrackedCmd.Dir = root
	if out, err := untrackedCmd.Output(); err == nil {
		files := strings.Split(strings.TrimSpace(string(out)), "\n")
		for _, f := range files {
			if f == "" {
				continue
			}
			h.Write([]byte(f))
			if content, err := os.ReadFile(filepath.Join(root, f)); err == nil {
				h.Write(content)
			}
		}
	}

	return hex.EncodeToString(h.Sum(nil)), nil
}

// filesystemSourceHash walks the workspace and hashes all relevant files.
// Uses a worker pool for parallel file hashing.
func filesystemSourceHash(root string) (string, error) {
	var files []string

	err := filepath.Walk(root, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil // skip unreadable
		}
		if info.IsDir() {
			base := info.Name()
			// Skip heavy directories
			if base == "target" || base == ".git" || base == "node_modules" ||
				base == ".nix" || base == "archive" || base == ".lci" {
				return filepath.SkipDir
			}
			return nil
		}
		name := info.Name()
		if strings.HasSuffix(name, ".rs") || name == "Cargo.toml" || name == "Cargo.lock" {
			files = append(files, path)
		}
		return nil
	})
	if err != nil {
		return "", err
	}

	sort.Strings(files) // deterministic order

	// Parallel hashing with worker pool
	type result struct {
		path string
		hash []byte
	}

	const workers = 8
	ch := make(chan string, len(files))
	results := make(chan result, len(files))

	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for path := range ch {
				f, err := os.Open(path)
				if err != nil {
					continue
				}
				h := sha256.New()
				_, copyErr := io.Copy(h, f)
				f.Close()
				if copyErr != nil {
					// Skip on read failure — a partial hash would silently
					// invalidate the cache key for this file's content.
					continue
				}
				results <- result{path: path, hash: h.Sum(nil)}
			}
		}()
	}

	for _, f := range files {
		ch <- f
	}
	close(ch)

	go func() {
		wg.Wait()
		close(results)
	}()

	// Collect per-file hashes into sorted map
	fileHashes := make(map[string][]byte)
	for r := range results {
		fileHashes[r.path] = r.hash
	}

	// Combine into final hash (sorted by path for determinism)
	combined := sha256.New()
	for _, f := range files {
		if h, ok := fileHashes[f]; ok {
			combined.Write([]byte(f))
			combined.Write(h)
		}
	}

	return hex.EncodeToString(combined.Sum(nil)), nil
}
