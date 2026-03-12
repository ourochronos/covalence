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
		client := newClient()
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
		client := newClient()
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

		client := newClient()
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
		client := newClient()
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
		client := newClient()
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

var adminAuditCmd = &cobra.Command{
	Use:   "audit",
	Short: "Run configuration audit (sidecar health + warnings)",
	RunE: func(cmd *cobra.Command, args []string) error {
		client := newClient()
		var result map[string]interface{}
		if err := client.Post("/admin/config-audit", nil, &result); err != nil {
			return fmt.Errorf("API error: %w", err)
		}

		if jsonOutput {
			return internal.PrintJSON(result)
		}

		// Print sidecars
		fmt.Println("=== Sidecar Health ===")
		if sidecars, ok := result["sidecars"].([]interface{}); ok {
			for _, s := range sidecars {
				sc, ok := s.(map[string]interface{})
				if !ok {
					continue
				}
				name := getString(sc, "name")
				configured := false
				if v, ok := sc["configured"].(bool); ok {
					configured = v
				}
				reachable := false
				if v, ok := sc["reachable"].(bool); ok {
					reachable = v
				}

				status := "not configured"
				if configured && reachable {
					status = "OK"
				} else if configured {
					status = "UNREACHABLE"
				}
				fmt.Printf("  %-12s %s\n", name+":", status)
			}
		}

		// Print warnings
		if warnings, ok := result["warnings"].([]interface{}); ok && len(warnings) > 0 {
			fmt.Println("\n=== Warnings ===")
			for _, w := range warnings {
				if ws, ok := w.(string); ok {
					fmt.Printf("  - %s\n", ws)
				}
			}
		} else {
			fmt.Println("\nNo warnings.")
		}

		return nil
	},
}

func init() {
	adminCmd.AddCommand(adminHealthCmd)
	adminCmd.AddCommand(adminReloadCmd)
	adminCmd.AddCommand(adminConsolidateCmd)
	adminCmd.AddCommand(adminMetricsCmd)
	adminCmd.AddCommand(adminPublishCmd)
	adminCmd.AddCommand(adminAuditCmd)
	rootCmd.AddCommand(adminCmd)
}
