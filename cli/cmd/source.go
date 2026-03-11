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

		client := internal.NewClient(apiURL)
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
		client := internal.NewClient(apiURL)
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
		client := internal.NewClient(apiURL)
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
		client := internal.NewClient(apiURL)
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
		client := internal.NewClient(apiURL)
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
			content := getString(c, "content")
			if len(content) > 60 {
				content = content[:60] + "..."
			}
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

func init() {
	sourceListCmd.Flags().IntVar(&sourceListLimit, "limit", 20,
		"Maximum sources to return")
	sourceCmd.AddCommand(sourceAddCmd)
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
