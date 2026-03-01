package cmd

import (
	"fmt"
	"os"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	apiURL   string
	jsonMode bool
	client   *internal.Client
)

var rootCmd = &cobra.Command{
	Use:   "cov",
	Short: "Covalence CLI — interact with the Covalence knowledge graph engine",
	Long:  `cov is the command-line interface for the Covalence knowledge graph REST API.`,
	PersistentPreRun: func(cmd *cobra.Command, args []string) {
		client = internal.NewClient(apiURL)
	},
}

func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func init() {
	defaultURL := os.Getenv("COVALENCE_API_URL")
	if defaultURL == "" {
		defaultURL = "http://localhost:8430"
	}
	rootCmd.PersistentFlags().StringVar(&apiURL, "api-url", defaultURL, "Covalence API base URL (env: COVALENCE_API_URL)")
	rootCmd.PersistentFlags().BoolVar(&jsonMode, "json", false, "Output raw JSON response")
}
