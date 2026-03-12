package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	searchStrategy    string
	searchLimit       int
	searchMinConfidence float64
	searchNodeTypes   []string
	searchMode        string
	searchGranularity string
)

var searchCmd = &cobra.Command{
	Use:   "search [query]",
	Short: "Search the knowledge base",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)

		body := map[string]interface{}{
			"query": args[0],
			"limit": searchLimit,
		}
		if searchStrategy != "auto" {
			body["strategy"] = searchStrategy
		}
		if searchMinConfidence > 0 {
			body["min_confidence"] = searchMinConfidence
		}
		if len(searchNodeTypes) > 0 {
			body["node_types"] = searchNodeTypes
		}
		if searchMode != "results" {
			body["mode"] = searchMode
		}
		if searchGranularity != "section" {
			body["granularity"] = searchGranularity
		}

		if searchMode == "context" {
			return handleContextMode(client, body)
		}

		return handleResultsMode(client, body)
	},
}

func handleResultsMode(client *internal.Client, body map[string]interface{}) error {
	var result []map[string]interface{}
	if err := client.Post("/search", body, &result); err != nil {
		return fmt.Errorf("API error: %w", err)
	}

	if jsonOutput {
		return internal.PrintJSON(result)
	}

	headers := []string{"Rank", "Score", "ID", "Name", "Type", "Confidence"}
	rows := make([][]string, 0, len(result))
	for i, r := range result {
		conf := ""
		if c := getFloat(r, "confidence"); c > 0 {
			conf = fmt.Sprintf("%.2f", c)
		}
		rows = append(rows, []string{
			fmt.Sprintf("%d", i+1),
			fmt.Sprintf("%.4f", getFloat(r, "fused_score")),
			shortID(getString(r, "id")),
			getString(r, "name"),
			getString(r, "entity_type"),
			conf,
		})
	}
	internal.PrintTable(headers, rows)

	// Print content for results that have it.
	for i, r := range result {
		content := getString(r, "content")
		if content != "" {
			fmt.Printf("\n--- [%d] %s ---\n", i+1, getString(r, "name"))
			if len(content) > 500 {
				fmt.Printf("%s...\n", content[:500])
			} else {
				fmt.Println(content)
			}
		}
	}
	return nil
}

func handleContextMode(client *internal.Client, body map[string]interface{}) error {
	var result map[string]interface{}
	if err := client.Post("/search", body, &result); err != nil {
		return fmt.Errorf("API error: %w", err)
	}

	if jsonOutput {
		return internal.PrintJSON(result)
	}

	items, ok := result["items"].([]interface{})
	if !ok {
		fmt.Println("No context items returned.")
		return nil
	}

	totalTokens := getFloat(result, "total_tokens")
	dropped := getFloat(result, "items_dropped")
	deduped := getFloat(result, "duplicates_removed")

	fmt.Printf("Context: %d items, %d tokens",
		len(items), int(totalTokens))
	if dropped > 0 {
		fmt.Printf(", %d dropped", int(dropped))
	}
	if deduped > 0 {
		fmt.Printf(", %d deduplicated", int(deduped))
	}
	fmt.Println()

	for _, raw := range items {
		item, ok := raw.(map[string]interface{})
		if !ok {
			continue
		}
		ref_num := getFloat(item, "ref_number")
		content := getString(item, "content")
		title := getString(item, "source_title")
		score := getFloat(item, "score")

		header := fmt.Sprintf("[%.0f]", ref_num)
		if title != "" {
			header += fmt.Sprintf(" %s", title)
		}
		header += fmt.Sprintf(" (score: %.4f)", score)
		fmt.Printf("\n%s\n", header)
		fmt.Println(content)
	}
	return nil
}

func init() {
	searchCmd.Flags().StringVar(&searchStrategy, "strategy", "auto",
		"Search strategy (auto, balanced, precise, exploratory, recent, graph_first, global)")
	searchCmd.Flags().IntVar(&searchLimit, "limit", 10,
		"Maximum results to return")
	searchCmd.Flags().Float64Var(&searchMinConfidence, "min-confidence", 0,
		"Minimum confidence threshold (0.0-1.0)")
	searchCmd.Flags().StringSliceVar(&searchNodeTypes, "node-types", nil,
		"Filter by node types (comma-separated)")
	searchCmd.Flags().StringVar(&searchMode, "mode", "results",
		"Delivery mode: results (default) or context")
	searchCmd.Flags().StringVar(&searchGranularity, "granularity", "section",
		"Content granularity: section (default), paragraph, or source")
	rootCmd.AddCommand(searchCmd)
}

func getFloat(m map[string]interface{}, key string) float64 {
	if v, ok := m[key]; ok {
		if f, ok := v.(float64); ok {
			return f
		}
	}
	return 0.0
}
