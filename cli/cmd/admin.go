package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var adminCmd = &cobra.Command{
	Use:   "admin",
	Short: "Administrative operations",
	Long:  "Reload graph, trigger consolidation, check health.",
}

var adminHealthCmd = &cobra.Command{
	Use:   "health",
	Short: "Check engine health",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Get("/admin/health", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Status:  %s\n", getString(result, "status"))
		fmt.Printf("Service: %s\n", getString(result, "service"))
		fmt.Printf("Version: %s\n", getString(result, "version"))
		return nil
	},
}

var adminReloadCmd = &cobra.Command{
	Use:   "reload",
	Short: "Force graph sidecar reload",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Post("/admin/graph/reload", nil, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Println("Graph sidecar reload triggered")
		return nil
	},
}

var adminConsolidateCmd = &cobra.Command{
	Use:   "consolidate [tier]",
	Short: "Trigger consolidation",
	Long:  "Trigger consolidation. Tier must be 'batch' or 'deep'.",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		tier := args[0]
		if tier != "batch" && tier != "deep" {
			return fmt.Errorf("tier must be 'batch' or 'deep', got '%s'", tier)
		}

		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		path := fmt.Sprintf("/admin/consolidate?tier=%s", tier)
		if err := client.Post(path, nil, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Consolidation (%s) triggered\n", tier)
		return nil
	},
}

var adminMetricsCmd = &cobra.Command{
	Use:   "metrics",
	Short: "Show engine metrics",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Get("/admin/metrics", &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Graph Nodes: %s\n", getString(result, "graph_nodes"))
		fmt.Printf("Graph Edges: %s\n", getString(result, "graph_edges"))
		fmt.Printf("Sources:     %s\n", getString(result, "source_count"))
		return nil
	},
}

var adminPublishCmd = &cobra.Command{
	Use:   "publish [source-id]",
	Short: "Publish a source (promote clearance level)",
	Args:  cobra.ExactArgs(1),
	RunE: func(cmd *cobra.Command, args []string) error {
		client := internal.NewClient(apiURL)
		var result map[string]interface{}
		if err := client.Post("/admin/publish/"+args[0], nil, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		fmt.Printf("Source %s published\n", shortID(args[0]))
		return nil
	},
}

func init() {
	adminCmd.AddCommand(adminHealthCmd)
	adminCmd.AddCommand(adminReloadCmd)
	adminCmd.AddCommand(adminConsolidateCmd)
	adminCmd.AddCommand(adminMetricsCmd)
	adminCmd.AddCommand(adminPublishCmd)
	rootCmd.AddCommand(adminCmd)
}
