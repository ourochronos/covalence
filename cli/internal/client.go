package internal

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"time"
)

type Client struct {
	BaseURL    string
	HTTPClient *http.Client
}

type Response struct {
	StatusCode int
	Body       []byte
}

func NewClient(baseURL string) *Client {
	return &Client{
		BaseURL: baseURL,
		HTTPClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

func (c *Client) do(method, path string, body interface{}, params url.Values) (*Response, error) {
	var bodyReader io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return nil, fmt.Errorf("marshal body: %w", err)
		}
		bodyReader = bytes.NewReader(b)
	}

	fullURL := c.BaseURL + path
	if params != nil && len(params) > 0 {
		fullURL += "?" + params.Encode()
	}

	req, err := http.NewRequest(method, fullURL, bodyReader)
	if err != nil {
		return nil, fmt.Errorf("create request: %w", err)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	req.Header.Set("Accept", "application/json")

	resp, err := c.HTTPClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("HTTP %s %s: %w", method, path, err)
	}
	defer resp.Body.Close()

	respBody, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("read response: %w", err)
	}

	r := &Response{StatusCode: resp.StatusCode, Body: respBody}
	if resp.StatusCode >= 400 {
		return r, fmt.Errorf("server error %d: %s", resp.StatusCode, string(respBody))
	}
	return r, nil
}

func (c *Client) Post(path string, body interface{}) (*Response, error) {
	return c.do("POST", path, body, nil)
}

func (c *Client) Get(path string, params url.Values) (*Response, error) {
	return c.do("GET", path, nil, params)
}

func (c *Client) Patch(path string, body interface{}) (*Response, error) {
	return c.do("PATCH", path, body, nil)
}

func (c *Client) Delete(path string) (*Response, error) {
	return c.do("DELETE", path, nil, nil)
}
