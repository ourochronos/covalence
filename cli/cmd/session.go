package cmd

import (
	"fmt"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var sessionCmd = &cobra.Command{
	Use:   "session",
	Short: "Manage sessions",
}

var sessionCreateCmd = &cobra.Command{
	Use:   "create",
	Short: "Create a new session",
	Run: func(cmd *cobra.Command, args []string) {
		label, _ := cmd.Flags().GetString("label")
		kind, _ := cmd.Flags().GetString("kind")

		body := map[string]interface{}{}
		if label != "" {
			body["label"] = label
		}
		if kind != "" {
			body["kind"] = kind
		}

		resp, err := client.Post("/sessions", body)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Println("Session created:")
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sessionListCmd = &cobra.Command{
	Use:   "list",
	Short: "List sessions",
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/sessions", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sessionGetCmd = &cobra.Command{
	Use:   "get <id>",
	Short: "Get a session by ID",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		resp, err := client.Get("/sessions/"+args[0], nil)
		if err != nil {
			internal.Die("%v", err)
		}
		internal.ParseAndPrint(resp.Body, jsonMode)
	},
}

var sessionCloseCmd = &cobra.Command{
	Use:   "close <id>",
	Short: "Close a session",
	Args:  cobra.ExactArgs(1),
	Run: func(cmd *cobra.Command, args []string) {
		_, err := client.Post("/sessions/"+args[0]+"/close", nil)
		if err != nil {
			internal.Die("%v", err)
		}
		fmt.Printf("Session %s closed.\n", args[0])
	},
}

func init() {
	sessionCmd.AddCommand(sessionCreateCmd, sessionListCmd, sessionGetCmd, sessionCloseCmd)

	sessionCreateCmd.Flags().String("label", "", "Optional session label")
	sessionCreateCmd.Flags().String("kind", "", "Session kind")

	rootCmd.AddCommand(sessionCmd)
}
