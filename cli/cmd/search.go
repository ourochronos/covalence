package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	searchStrategy      string
	searchLimit         int
	searchMinConfidence float64
	searchNodeTypes     []string
)

var searchCmd = &cobra.Command{
	Use:   "search [query]",
	Short: "Search the knowledge base",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)

		body := map[string]interface{}{
			"query":    args[0],
			"strategy": searchStrategy,
			"limit":    searchLimit,
		}
		if searchMinConfidence > 0 {
			body["min_confidence"] = searchMinConfidence
		}
		if len(searchNodeTypes) > 0 {
			body["node_types"] = searchNodeTypes
		}

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
		return nil
	},
}

func init() {
	searchCmd.Flags().StringVar(&searchStrategy, "strategy", "balanced",
		"Search strategy (balanced, precise, exploratory, recent, graph_first, global)")
	searchCmd.Flags().IntVar(&searchLimit, "limit", 10,
		"Maximum results to return")
	searchCmd.Flags().Float64Var(&searchMinConfidence, "min-confidence", 0,
		"Minimum confidence threshold (0.0-1.0)")
	searchCmd.Flags().StringSliceVar(&searchNodeTypes, "node-types", nil,
		"Filter by node types (comma-separated)")
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
