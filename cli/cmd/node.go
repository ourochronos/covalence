package cmd

import (
	"fmt"
	"net/url"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	nodeListType  string
	nodeListLimit int
	neighborHops  int
)

var nodeCmd = &cobra.Command{
	Use:   "node",
	Short: "Manage graph nodes",
	Long:  "List, inspect, and manipulate graph nodes.",
}

var nodeListCmd = &cobra.Command{
	Use:   "list",
	Short: "List nodes",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		path := fmt.Sprintf("/nodes?limit=%d", nodeListLimit)
		if nodeListType != "" {
			path += "&type=" + url.QueryEscape(nodeListType)
		}

		var result []map[string]interface{}
		if err := client.Get(path, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Name", "Type", "Mentions"}
		rows := make([][]string, 0, len(result))
		for _, n := range result {
			rows = append(rows, []string{
				shortID(getString(n, "id")),
				getString(n, "canonical_name"),
				getString(n, "node_type"),
				getString(n, "mention_count"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var nodeGetCmd = &cobra.Command{
	Use:   "get [id]",
	Short: "Get node details",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Get("/nodes/"+args[0], &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("ID:            %s\n", getString(result, "id"))
		fmt.Printf("Name:          %s\n", getString(result, "canonical_name"))
		fmt.Printf("Type:          %s\n", getString(result, "node_type"))
		fmt.Printf("Description:   %s\n", getString(result, "description"))
		fmt.Printf("Clearance:     %s\n", getString(result, "clearance_level"))
		fmt.Printf("First Seen:    %s\n", getString(result, "first_seen"))
		fmt.Printf("Last Seen:     %s\n", getString(result, "last_seen"))
		fmt.Printf("Mention Count: %s\n", getString(result, "mention_count"))
		return nil
	},
}

var nodeNeighborhoodCmd = &cobra.Command{
	Use:   "neighborhood [id]",
	Short: "Show node neighborhood",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		path := fmt.Sprintf("/nodes/%s/neighborhood?hops=%d", args[0], neighborHops)

		var result []map[string]interface{}
		if err := client.Get(path, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Name", "Type", "Mentions"}
		rows := make([][]string, 0, len(result))
		for _, n := range result {
			rows = append(rows, []string{
				shortID(getString(n, "id")),
				getString(n, "canonical_name"),
				getString(n, "node_type"),
				getString(n, "mention_count"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var nodeProvenanceCmd = &cobra.Command{
	Use:   "provenance [id]",
	Short: "Show node provenance chain",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Get("/nodes/"+args[0]+"/provenance", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Node ID:      %s\n", getString(result, "node_id"))
		fmt.Printf("Extractions:  %s\n", getString(result, "extraction_count"))
		fmt.Printf("Chunks:       %s\n", getString(result, "chunk_count"))
		fmt.Printf("Sources:      %s\n", getString(result, "source_count"))
		return nil
	},
}

func init() {
	nodeListCmd.Flags().StringVar(&nodeListType, "type", "",
		"Filter by node type")
	nodeListCmd.Flags().IntVar(&nodeListLimit, "limit", 20,
		"Maximum nodes to return")
	nodeNeighborhoodCmd.Flags().IntVar(&neighborHops, "hops", 2,
		"Number of hops to traverse")
	nodeCmd.AddCommand(nodeListCmd)
	nodeCmd.AddCommand(nodeGetCmd)
	nodeCmd.AddCommand(nodeNeighborhoodCmd)
	nodeCmd.AddCommand(nodeProvenanceCmd)
	rootCmd.AddCommand(nodeCmd)
}
