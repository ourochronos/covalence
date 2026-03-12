package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var graphCmd = &cobra.Command{
	Use:   "graph",
	Short: "Graph operations",
	Long:  "Inspect graph statistics, communities, and topology.",
}

var graphStatsCmd = &cobra.Command{
	Use:   "stats",
	Short: "Show graph statistics",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Get("/graph/stats", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Nodes:      %s\n", getString(result, "node_count"))
		fmt.Printf("Edges:      %s\n", getString(result, "edge_count"))
		fmt.Printf("Density:    %s\n", getString(result, "density"))
		fmt.Printf("Components: %s\n", getString(result, "component_count"))
		return nil
	},
}

var graphCommunitiesCmd = &cobra.Command{
	Use:   "communities",
	Short: "List detected communities",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result []map[string]interface{}
		if err := client.Get("/graph/communities", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		headers := []string{"ID", "Nodes", "Label", "Coherence"}
		rows := make([][]string, 0, len(result))
		for _, c := range result {
			nodeIDs, _ := c["node_ids"].([]interface{})
			rows = append(rows, []string{
				getString(c, "id"),
				fmt.Sprintf("%d", len(nodeIDs)),
				getString(c, "label"),
				getString(c, "coherence"),
			})
		}
		internal.PrintTable(headers, rows)
		return nil
	},
}

var graphTopologyCmd = &cobra.Command{
	Use:   "topology",
	Short: "Show domain topology map",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Get("/graph/topology", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Total Nodes: %s\n", getString(result, "total_nodes"))
		fmt.Printf("Total Edges: %s\n", getString(result, "total_edges"))

		if domains, ok := result["domains"].([]interface{}); ok {
			fmt.Printf("\nDomains (%d):\n", len(domains))
			for _, d := range domains {
				if dm, ok := d.(map[string]interface{}); ok {
					fmt.Printf("  Community %s: %s nodes, coherence=%s, avg_pagerank=%s\n",
						getString(dm, "community_id"),
						getString(dm, "node_count"),
						getString(dm, "coherence"),
						getString(dm, "avg_pagerank"),
					)
				}
			}
		}

		if links, ok := result["links"].([]interface{}); ok && len(links) > 0 {
			fmt.Printf("\nInter-domain Links (%d):\n", len(links))
			for _, l := range links {
				if lm, ok := l.(map[string]interface{}); ok {
					fmt.Printf("  %s -> %s: %s bridges\n",
						getString(lm, "source_domain"),
						getString(lm, "target_domain"),
						getString(lm, "bridge_count"),
					)
				}
			}
		}
		return nil
	},
}

func init() {
	graphCmd.AddCommand(graphStatsCmd)
	graphCmd.AddCommand(graphCommunitiesCmd)
	graphCmd.AddCommand(graphTopologyCmd)
	rootCmd.AddCommand(graphCmd)
}
