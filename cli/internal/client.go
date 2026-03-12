// Package internal provides HTTP client and output utilities for the CLI.
package internal

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

// Client wraps HTTP communication with the Covalence API.
type Client struct {
	BaseURL    string
	APIKey     string
	HTTPClient *http.Client
}

// NewClient creates a new API client.
// The baseURL should be the engine root (e.g. http://localhost:8431);
// /api/v1 is appended automatically.
func NewClient(baseURL string) *Client {
	return &Client{
		BaseURL: strings.TrimRight(baseURL, "/") + "/api/v1",
		HTTPClient: &http.Client{
			Timeout: 15 * time.Minute,
		},
	}
}

// NewClientWithKey creates a new API client with authentication.
func NewClientWithKey(baseURL, apiKey string) *Client {
	c := NewClient(baseURL)
	c.APIKey = apiKey
	return c
}

// Get performs a GET request and decodes the JSON response.
func (c *Client) Get(path string, result interface{}) error {
	req, err := http.NewRequest(http.MethodGet, c.BaseURL+path, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	c.setAuth(req)

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		body, readErr := io.ReadAll(resp.Body)
		if readErr != nil {
			return fmt.Errorf("API error %d (failed to read body: %v)", resp.StatusCode, readErr)
		}
		return fmt.Errorf("API error %d: %s", resp.StatusCode, string(body))
	}

	return json.NewDecoder(resp.Body).Decode(result)
}

// Post performs a POST request with a JSON body and decodes the JSON response.
func (c *Client) Post(path string, body interface{}, result interface{}) error {
	jsonBody, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("failed to encode request body: %w", err)
	}

	req, err := http.NewRequest(http.MethodPost, c.BaseURL+path, bytes.NewReader(jsonBody))
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	c.setAuth(req)

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		respBody, readErr := io.ReadAll(resp.Body)
		if readErr != nil {
			return fmt.Errorf("API error %d (failed to read body: %v)", resp.StatusCode, readErr)
		}
		return fmt.Errorf("API error %d: %s", resp.StatusCode, string(respBody))
	}

	return json.NewDecoder(resp.Body).Decode(result)
}

// Delete performs a DELETE request and decodes the JSON response.
// Handles 204 No Content gracefully (no body to decode).
func (c *Client) Delete(path string, result interface{}) error {
	req, err := http.NewRequest(http.MethodDelete, c.BaseURL+path, nil)
	if err != nil {
		return fmt.Errorf("failed to create request: %w", err)
	}
	c.setAuth(req)

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return fmt.Errorf("request failed: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		body, readErr := io.ReadAll(resp.Body)
		if readErr != nil {
			return fmt.Errorf("API error %d (failed to read body: %v)", resp.StatusCode, readErr)
		}
		return fmt.Errorf("API error %d: %s", resp.StatusCode, string(body))
	}

	// 204 No Content has no body to decode.
	if resp.StatusCode == http.StatusNoContent {
		return nil
	}

	return json.NewDecoder(resp.Body).Decode(result)
}

// setAuth adds the Authorization header if an API key is configured.
func (c *Client) setAuth(req *http.Request) {
	if c.APIKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.APIKey)
	}
}
