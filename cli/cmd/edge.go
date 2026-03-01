package cmd

import (
	"fmt"
	"net/url"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var edgeCmd = &cobra.Command{
	Use:   "edge",
	Short: "Manage knowledge graph edges",
}

var edgeCreateCmd = &cobra.Command{
	Use:   "create",
	Short: "Create a new edge between two nodes",
	Run: func(cmd *cobra.Command, args []string) {
		from, _ := cmd.Flags().GetString("from")
		to, _ := cmd.Flags().GetString("to")
		label, _ := cmd.Flags().GetString("label")
		confidence, _ := cmd.Flags().GetFloat64("confidence")

		if from == "" || to == "" || label == "" {
			internal.Die("--from, --to, and --label are required")
		}

		body := map[string]interface{}{
			"source_id": from,
			"target_id": to,
			"label":     label,
		}
		if cmd.Flags().Changed("confidence") {
			body["confidence"] = confidence
		}

		resp, err := client.Post("/edges", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Edge created:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var edgeListCmd = &cobra.Command{
	Use:   "list",
	Short: "List edges for a node",
	Run: func(cmd *cobra.Command, args []string) {
		node, _ := cmd.Flags().GetString("node")
		direction, _ := cmd.Flags().GetString("direction")
		label, _ := cmd.Flags().GetString("label")
		limit, _ := cmd.Flags().GetInt("limit")

		if node == "" {
			internal.Die("--node is required")
		}

		p := url.Values{}
		if direction != "" {
			p.Set("direction", direction)
		}
		if label != "" {
			p.Set("labels", label)
		}
		if limit > 0 {
			p.Set("limit", fmt.Sprintf("%d", limit))
		}

		// Engine route: GET /nodes/{id}/edges
		resp, err := client.Get("/nodes/"+node+"/edges", p)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var edgeDeleteCmd = &cobra.Command{
	Use:   "delete <id>",
	Short: "Delete an edge",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		_, err := client.Delete("/edges/" + args[0])
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Printf("Edge %s deleted.\n", args[0])
	},
}

func init() {
	edgeCmd.AddCommand(edgeCreateCmd, edgeListCmd, edgeDeleteCmd)

	edgeCreateCmd.Flags().String("from", "", "Source article/node ID (required)")
	edgeCreateCmd.Flags().String("to", "", "Target article/node ID (required)")
	edgeCreateCmd.Flags().String("label", "", "Edge label, e.g. CAUSES (required)")
	edgeCreateCmd.Flags().Float64("confidence", 1.0, "Edge confidence (0.0-1.0)")

	edgeListCmd.Flags().String("node", "", "Node ID to list edges for (required)")
	edgeListCmd.Flags().String("direction", "", "Direction: inbound or outbound")
	edgeListCmd.Flags().String("label", "", "Filter by label")
	edgeListCmd.Flags().Int("limit", 50, "Maximum results")

	rootCmd.AddCommand(edgeCmd)
}
