// Package client is the provider's client for the meter role's REST API
// (design §63 §17.2: every Connect method of `loams.meter.v1` mapped under
// /v1/meter/…). The paths and JSON names are the ones D2216 proposes to
// ME-svc; JSON uses protobuf's lowerCamelCase, as the transcoding writes it.
package client

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// Client calls one Loams deployment with a Loams API key. The key's
// principal needs `meter_admin` (agent principals can never hold it, §63
// §19).
type Client struct {
	base      string
	apiKey    string
	http      *http.Client
	userAgent string
}

// New returns a client for base (for example https://loams.example.ts.net).
func New(base, apiKey, userAgent string) (*Client, error) {
	u, err := url.Parse(base)
	if err != nil || (u.Scheme != "https" && u.Scheme != "http") || u.Host == "" {
		return nil, fmt.Errorf("endpoint %q is not an http(s) URL", base)
	}
	if strings.TrimSpace(apiKey) == "" {
		return nil, errors.New("no API key")
	}
	return &Client{
		base:      strings.TrimRight(base, "/"),
		apiKey:    apiKey,
		http:      &http.Client{Timeout: 60 * time.Second},
		userAgent: userAgent,
	}, nil
}

// APIError is a non-2xx answer: the Connect error's code and message.
type APIError struct {
	Status  int
	Code    string
	Message string
	Reason  string
}

func (e *APIError) Error() string {
	msg := e.Message
	if msg == "" {
		msg = http.StatusText(e.Status)
	}
	if e.Reason != "" {
		return fmt.Sprintf("%d %s (%s): %s", e.Status, e.Code, e.Reason, msg)
	}
	return fmt.Sprintf("%d %s: %s", e.Status, e.Code, msg)
}

// IsNotFound reports whether err is a 404 / not_found.
func IsNotFound(err error) bool {
	var e *APIError
	return errors.As(err, &e) && (e.Status == http.StatusNotFound || e.Code == "not_found")
}

func (c *Client) do(ctx context.Context, method, path string, in, out any) error {
	var body io.Reader
	if in != nil {
		b, err := json.Marshal(in)
		if err != nil {
			return err
		}
		body = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, body)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.apiKey)
	req.Header.Set("Accept", "application/json")
	if in != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.userAgent != "" {
		req.Header.Set("User-Agent", c.userAgent)
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer func() { _ = resp.Body.Close() }()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return err
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		e := &APIError{Status: resp.StatusCode}
		var ce struct {
			Code    string `json:"code"`
			Message string `json:"message"`
			Details []struct {
				Reason string `json:"reason"`
			} `json:"details"`
		}
		if json.Unmarshal(raw, &ce) == nil {
			e.Code, e.Message = ce.Code, ce.Message
			for _, d := range ce.Details {
				if d.Reason != "" {
					e.Reason = d.Reason
					break
				}
			}
		}
		return e
	}
	if out != nil && len(bytes.TrimSpace(raw)) > 0 {
		if err := json.Unmarshal(raw, out); err != nil {
			return fmt.Errorf("%s %s: unexpected answer: %w", method, path, err)
		}
	}
	return nil
}

func esc(s string) string { return url.PathEscape(s) }

// Meter is OpenMeter's meter definition (§63 §8.1).
type Meter struct {
	Slug          string            `json:"slug"`
	Name          string            `json:"name,omitempty"`
	Description   string            `json:"description,omitempty"`
	EventType     string            `json:"eventType"`
	Aggregation   string            `json:"aggregation"`
	ValueProperty string            `json:"valueProperty,omitempty"`
	GroupBy       map[string]string `json:"groupBy,omitempty"`
	EventFrom     string            `json:"eventFrom,omitempty"`
	Archived      bool              `json:"archived,omitempty"`
}

// CreateMeter is MeterService.CreateMeter.
func (c *Client) CreateMeter(ctx context.Context, m Meter) (*Meter, error) {
	var out Meter
	return &out, c.do(ctx, http.MethodPost, "/v1/meter/meters", m, &out)
}

// GetMeter is MeterService.GetMeter.
func (c *Client) GetMeter(ctx context.Context, slug string) (*Meter, error) {
	var out Meter
	return &out, c.do(ctx, http.MethodGet, "/v1/meter/meters/"+esc(slug), nil, &out)
}

// UpdateMeter changes a meter's display fields, the only ones that can change
// (a meter's semantics are immutable once used, §63 §17.1).
func (c *Client) UpdateMeter(ctx context.Context, slug, name, description string) (*Meter, error) {
	var out Meter
	in := map[string]string{"name": name, "description": description}
	return &out, c.do(ctx, http.MethodPatch, "/v1/meter/meters/"+esc(slug)+"?updateMask=name,description", in, &out)
}

// ArchiveMeter is MeterService.ArchiveMeter; meters are never deleted.
func (c *Client) ArchiveMeter(ctx context.Context, slug string) error {
	return c.do(ctx, http.MethodPost, "/v1/meter/meters/"+esc(slug)+":archive", struct{}{}, nil)
}

// Feature is what a customer is entitled to (§63 §9.2).
type Feature struct {
	Key       string            `json:"key"`
	Name      string            `json:"name,omitempty"`
	MeterSlug string            `json:"meterSlug,omitempty"`
	Filter    map[string]string `json:"filter,omitempty"`
	Unit      string            `json:"unit"`
	Archived  bool              `json:"archived,omitempty"`
}

// CreateFeature creates a feature.
func (c *Client) CreateFeature(ctx context.Context, f Feature) (*Feature, error) {
	var out Feature
	return &out, c.do(ctx, http.MethodPost, "/v1/meter/features", f, &out)
}

// GetFeature reads a feature.
func (c *Client) GetFeature(ctx context.Context, key string) (*Feature, error) {
	var out Feature
	return &out, c.do(ctx, http.MethodGet, "/v1/meter/features/"+esc(key), nil, &out)
}

// UpdateFeature changes a feature's display name.
func (c *Client) UpdateFeature(ctx context.Context, key, name string) (*Feature, error) {
	var out Feature
	return &out, c.do(ctx, http.MethodPatch, "/v1/meter/features/"+esc(key)+"?updateMask=name", map[string]string{"name": name}, &out)
}

// ArchiveFeature archives a feature; existing entitlements keep it.
func (c *Client) ArchiveFeature(ctx context.Context, key string) error {
	return c.do(ctx, http.MethodPost, "/v1/meter/features/"+esc(key)+":archive", struct{}{}, nil)
}

// Plan is one version of a plan (§63 §9.5); Definition is its YAML.
type Plan struct {
	Key        string `json:"key"`
	Version    int64  `json:"version"`
	Definition string `json:"definition,omitempty"`
}

// ApplyPlan is PlanService.ApplyPlan: it publishes a new version, or
// answers with the existing one when the same version and definition are
// applied again. The service refuses a changed definition under a version
// it already has.
func (c *Client) ApplyPlan(ctx context.Context, p Plan) (*Plan, error) {
	var out Plan
	return &out, c.do(ctx, http.MethodPost, "/v1/meter/plans:apply", p, &out)
}

// GetPlan reads a plan's latest version.
func (c *Client) GetPlan(ctx context.Context, key string) (*Plan, error) {
	var out Plan
	return &out, c.do(ctx, http.MethodGet, "/v1/meter/plans/"+esc(key), nil, &out)
}

// PlanRef names a plan version.
type PlanRef struct {
	Plan    string `json:"plan"`
	Version int64  `json:"version,omitempty"`
}

// Customer is the part of a customer the provider reads.
type Customer struct {
	Key  string   `json:"key"`
	Plan *PlanRef `json:"plan,omitempty"`
}

// AssignPlan is PlanService.AssignPlan; version 0 means the latest.
func (c *Client) AssignPlan(ctx context.Context, customer string, ref PlanRef) (*Customer, error) {
	var out Customer
	return &out, c.do(ctx, http.MethodPost, "/v1/meter/customers/"+esc(customer)+":assignPlan", ref, &out)
}

// UnassignPlan is PlanService.UnassignPlan.
func (c *Client) UnassignPlan(ctx context.Context, customer string) error {
	return c.do(ctx, http.MethodPost, "/v1/meter/customers/"+esc(customer)+":unassignPlan", struct{}{}, nil)
}

// GetCustomer is CustomerService.GetCustomer.
func (c *Client) GetCustomer(ctx context.Context, customer string) (*Customer, error) {
	var out Customer
	return &out, c.do(ctx, http.MethodGet, "/v1/meter/customers/"+esc(customer), nil, &out)
}
