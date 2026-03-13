package cmd

import (
	"encoding/base64"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var sourceListLimit int
var sourceAddTitle string
var sourceAddAuthor string
var sourceAddURI string
var sourceAddURLTitle string
var sourceAddURLAuthor string

var sourceCmd = &cobra.Command{
	Use:   "source",
	Short: "Manage sources",
	Long:  "Add, list, and inspect ingested sources.",
}

var sourceAddCmd = &cobra.Command{
	Use:   "add [path]",
	Short: "Ingest a source",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		filePath := args[0]
		content, err := os.ReadFile(filePath)
		if err != nil {
			return fmt.Errorf("failed to read file: %w", err)
		}

		mimeType := detectMIME(filePath)
		encoded := base64.StdEncoding.EncodeToString(content)

		body := map[string]interface{}{
			"source_type": "document",
			"content":     encoded,
			"mime":        mimeType,
			"metadata": map[string]string{
				"filename": filepath.Base(filePath),
			},
		}
		if sourceAddTitle != "" {
			body["title"] = sourceAddTitle
		}
		if sourceAddAuthor != "" {
			body["author"] = sourceAddAuthor
		}
		if sourceAddURI != "" {
			body["uri"] = sourceAddURI
		} else {
			// Auto-derive file:// URI from the path. Use the path
			// relative to the git repo root for portability; fall
			// back to the absolute path if outside a repo.
			absPath, _ := filepath.Abs(filePath)
			uri := "file://" + absPath
			if repoRoot := findRepoRoot(absPath); repoRoot != "" {
				if rel, err := filepath.Rel(repoRoot, absPath); err == nil {
					uri = "file://" + rel
				}
			}
			body["uri"] = uri
		}

		client := newClient()
		var result map[string]interface{}
		if err := client.Post("/sources", body, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		if id, ok := result["id"]; ok {
			fmt.Printf("Source ingested: %v\n", id)
		} else {
			fmt.Println("Source ingested successfully")
		}
		return nil
	},
}

var sourceListCmd = &cobra.Command{
	Use:   "list",
	Short: "List sources",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result []map[string]interface{}
		path := fmt.Sprintf("/sources?limit=%d", sourceListLimit)
		if err := client.Get(path, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Type", "Title", "Ingested At"}
		rows := make([][]string, 0, len(result))
		for _, s := range result {
			rows = append(rows, []string{
				shortID(getString(s, "id")),
				getString(s, "source_type"),
				getString(s, "title"),
				getString(s, "ingested_at"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var sourceGetCmd = &cobra.Command{
	Use:   "get [id]",
	Short: "Get source details",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Get("/sources/"+args[0], &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("ID:          %s\n", getString(result, "id"))
		fmt.Printf("Type:        %s\n", getString(result, "source_type"))
		fmt.Printf("Title:       %s\n", getString(result, "title"))
		fmt.Printf("URI:         %s\n", getString(result, "uri"))
		fmt.Printf("Author:      %s\n", getString(result, "author"))
		fmt.Printf("Clearance:   %s\n", getString(result, "clearance_level"))
		fmt.Printf("Ingested At: %s\n", getString(result, "ingested_at"))
		return nil
	},
}

var sourceDeleteCmd = &cobra.Command{
	Use:   "delete [id]",
	Short: "Delete a source",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Delete("/sources/"+args[0], &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Source %s deleted\n", shortID(args[0]))
		return nil
	},
}

var sourceChunksCmd = &cobra.Command{
	Use:   "chunks [id]",
	Short: "List chunks for a source",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result []map[string]interface{}
		if err := client.Get("/sources/"+args[0]+"/chunks", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Level", "Ordinal", "Tokens", "Content Preview"}
		rows := make([][]string, 0, len(result))
		for _, c := range result {
			content := truncateRunes(getString(c, "content"), 60)
			rows = append(rows, []string{
				shortID(getString(c, "id")),
				getString(c, "level"),
				getString(c, "ordinal"),
				getString(c, "token_count"),
				content,
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var sourceReprocessCmd = &cobra.Command{
	Use:   "reprocess [id]",
	Short: "Reprocess a source (re-chunk, re-embed, re-extract)",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Post("/sources/"+args[0]+"/reprocess", nil, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Source %s reprocessed\n", shortID(args[0]))
		if v, ok := result["chunks_deleted"]; ok {
			fmt.Printf("  Chunks deleted:  %v\n", v)
		}
		if v, ok := result["chunks_created"]; ok {
			fmt.Printf("  Chunks created:  %v\n", v)
		}
		if v, ok := result["content_version"]; ok {
			fmt.Printf("  Version:         %v\n", v)
		}
		return nil
	},
}

var sourceAddURLCmd = &cobra.Command{
	Use:   "add-url [url]",
	Short: "Ingest a source from a URL",
	Long:  "Fetch content from a URL and ingest it. The server auto-detects MIME type, source classification, and metadata.",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		url := args[0]

		body := map[string]interface{}{
			"url": url,
		}
		if sourceAddURLTitle != "" {
			body["title"] = sourceAddURLTitle
		}
		if sourceAddURLAuthor != "" {
			body["author"] = sourceAddURLAuthor
		}

		client := newClient()
		var result map[string]interface{}
		if err := client.Post("/sources", body, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		if id, ok := result["id"]; ok {
			fmt.Printf("Source ingested: %v\n", id)
		} else {
			fmt.Println("Source ingested successfully")
		}
		return nil
	},
}

func init() {
	sourceAddCmd.Flags().StringVar(&sourceAddTitle, "title", "",
		"Source title")
	sourceAddCmd.Flags().StringVar(&sourceAddAuthor, "author", "",
		"Source author")
	sourceAddCmd.Flags().StringVar(&sourceAddURI, "uri", "",
		"Source URI")
	sourceListCmd.Flags().IntVar(&sourceListLimit, "limit", 100,
		"Maximum sources to return")
	sourceAddURLCmd.Flags().StringVar(&sourceAddURLTitle, "title", "",
		"Override auto-detected title")
	sourceAddURLCmd.Flags().StringVar(&sourceAddURLAuthor, "author", "",
		"Override auto-detected author")
	sourceCmd.AddCommand(sourceAddCmd)
	sourceCmd.AddCommand(sourceAddURLCmd)
	sourceCmd.AddCommand(sourceReprocessCmd)
	sourceCmd.AddCommand(sourceListCmd)
	sourceCmd.AddCommand(sourceGetCmd)
	sourceCmd.AddCommand(sourceDeleteCmd)
	sourceCmd.AddCommand(sourceChunksCmd)
	rootCmd.AddCommand(sourceCmd)
}

func detectMIME(path string) string {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".md":
		return "text/markdown"
	case ".txt":
		return "text/plain"
	case ".html", ".htm":
		return "text/html"
	case ".json":
		return "application/json"
	case ".pdf":
		return "application/pdf"
	case ".rs":
		return "text/x-rust"
	case ".py":
		return "text/x-python"
	default:
		return "text/plain"
	}
}

// findRepoRoot walks up from dir looking for a .git directory.
// Returns the repo root path or "" if not inside a git repo.
func findRepoRoot(dir string) string {
	for {
		if _, err := os.Stat(filepath.Join(dir, ".git")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return ""
		}
		dir = parent
	}
}

func shortID(id string) string {
	if len(id) >= 8 {
		return id[:8]
	}
	return id
}

func getString(m map[string]interface{}, key string) string {
	if v, ok := m[key]; ok && v != nil {
		return fmt.Sprintf("%v", v)
	}
	return ""
}

// truncateRunes truncates a string to maxRunes runes, appending
// "..." if truncation occurred. Safe for multi-byte UTF-8.
func truncateRunes(s string, maxRunes int) string {
	runes := []rune(s)
	if len(runes) <= maxRunes {
		return s
	}
	return string(runes[:maxRunes]) + "..."
}
