// Package cmd implements the Cobra CLI commands for Covalence.
package cmd

import (
	"fmt"
	"os"

	"github.com/ourochronos/covalence/cli/internal"
	"github.com/spf13/cobra"
)

var (
	apiURL     string
	apiKey     string
	jsonOutput bool
)

var rootCmd = &cobra.Command{
	Use:   "cove",
	Short: "Covalence knowledge engine CLI",
	Long:  "cove is a CLI for interacting with the Covalence knowledge engine.",
}

// Execute runs the root command.
func Execute() {
	if err := rootCmd.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// defaultAPIURL returns the API URL the CLI should target.
//
// Order of precedence: COVALENCE_API_URL env var, then a sensible
// localhost default. Without the env-var fallback, running cove
// against a non-default port (e.g. dev on 8441) requires passing
// --api-url on every invocation.
func defaultAPIURL() string {
	if v := os.Getenv("COVALENCE_API_URL"); v != "" {
		return v
	}
	return "http://localhost:8431"
}

func init() {
	rootCmd.PersistentFlags().StringVar(&apiURL, "api-url", defaultAPIURL(), "Covalence API URL (or set COVALENCE_API_URL)")
	rootCmd.PersistentFlags().StringVar(&apiKey, "api-key", "", "API key for authentication (or set COVALENCE_API_KEY)")
	rootCmd.PersistentFlags().BoolVar(&jsonOutput, "json", false, "Output in JSON format")
}

// newClient creates an API client with authentication if configured.
func newClient() *internal.Client {
	key := apiKey
	if key == "" {
		key = os.Getenv("COVALENCE_API_KEY")
	}
	return internal.NewClientWithKey(apiURL, key)
}
