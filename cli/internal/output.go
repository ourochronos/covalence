package internal

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"
)

// PrintJSON outputs raw JSON, pretty-printed.
func PrintJSON(data []byte) {
	var v interface{}
	if err := json.Unmarshal(data, &v); err != nil {
		fmt.Fprintf(os.Stdout, "%s\n", data)
		return
	}
	out, _ := json.MarshalIndent(v, "", "  ")
	fmt.Println(string(out))
}

// PrintMap prints a map as key: value pairs, sorted by key.
func PrintMap(m map[string]interface{}, indent string) {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		v := m[k]
		switch val := v.(type) {
		case map[string]interface{}:
			fmt.Printf("%s%-22s\n", indent, k+":")
			PrintMap(val, indent+"  ")
		case []interface{}:
			if len(val) == 0 {
				fmt.Printf("%s%-22s []\n", indent, k+":")
			} else {
				fmt.Printf("%s%-22s\n", indent, k+":")
				for i, item := range val {
					if m2, ok := item.(map[string]interface{}); ok {
						fmt.Printf("%s  [%d]\n", indent, i)
						PrintMap(m2, indent+"    ")
					} else {
						fmt.Printf("%s  - %v\n", indent, item)
					}
				}
			}
		default:
			fmt.Printf("%s%-22s %v\n", indent, k+":", v)
		}
	}
}

// PrintList prints a slice of items as a numbered list.
func PrintList(items []interface{}) {
	if len(items) == 0 {
		fmt.Println("(no results)")
		return
	}
	fmt.Printf("Found %d item(s):\n", len(items))
	for i, item := range items {
		fmt.Printf("\n[%d]\n", i+1)
		if m, ok := item.(map[string]interface{}); ok {
			PrintMap(m, "  ")
		} else {
			fmt.Printf("  %v\n", item)
		}
	}
}

// ParseAndPrint unmarshals JSON response and prints it.
// Unwraps common API envelope patterns ({data: ...}).
func ParseAndPrint(data []byte, jsonMode bool) {
	if jsonMode {
		PrintJSON(data)
		return
	}
	var v interface{}
	if err := json.Unmarshal(data, &v); err != nil {
		fmt.Println(string(data))
		return
	}
	// Unwrap {"data": ...} envelope
	if m, ok := v.(map[string]interface{}); ok {
		if inner, ok := m["data"]; ok {
			v = inner
		}
	}
	switch val := v.(type) {
	case []interface{}:
		PrintList(val)
	case map[string]interface{}:
		PrintMap(val, "  ")
	default:
		fmt.Printf("  %v\n", v)
	}
}

// Die prints an error and exits with code 1.
func Die(format string, args ...interface{}) {
	msg := fmt.Sprintf(format, args...)
	fmt.Fprintf(os.Stderr, "Error: %s\n", msg)
	os.Exit(1)
}
