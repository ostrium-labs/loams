package client

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

type seen struct {
	method, path, query, auth, body string
}

func fake(t *testing.T, status int, answer string) (*Client, *[]seen) {
	t.Helper()
	var got []seen
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		got = append(got, seen{r.Method, r.URL.EscapedPath(), r.URL.RawQuery, r.Header.Get("Authorization"), string(b)})
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(status)
		_, _ = io.WriteString(w, answer)
	}))
	t.Cleanup(srv.Close)
	c, err := New(srv.URL, "lk_test_key", "terraform-provider-loams/test")
	if err != nil {
		t.Fatal(err)
	}
	return c, &got
}

func TestCreateMeterSendsTheDefinition(t *testing.T) {
	c, got := fake(t, 200, `{"slug":"house_cpu_ms","eventType":"io.loams.dev.usage.house.query.v1","aggregation":"SUM","valueProperty":"$.cpu_ms"}`)
	m, err := c.CreateMeter(context.Background(), Meter{
		Slug: "house_cpu_ms", EventType: "io.loams.dev.usage.house.query.v1",
		Aggregation: "SUM", ValueProperty: "$.cpu_ms", GroupBy: map[string]string{"ns": "$.namespace"},
	})
	if err != nil {
		t.Fatal(err)
	}
	if m.Slug != "house_cpu_ms" || m.Aggregation != "SUM" {
		t.Fatalf("answer: %+v", m)
	}
	r := (*got)[0]
	if r.method != "POST" || r.path != "/v1/meter/meters" || r.auth != "Bearer lk_test_key" {
		t.Fatalf("request: %+v", r)
	}
	var body map[string]any
	if err := json.Unmarshal([]byte(r.body), &body); err != nil {
		t.Fatal(err)
	}
	if body["eventType"] != "io.loams.dev.usage.house.query.v1" || body["groupBy"].(map[string]any)["ns"] != "$.namespace" {
		t.Fatalf("body: %s", r.body)
	}
}

func TestPathsAreEscapedAndVerbsAreCustomMethods(t *testing.T) {
	c, got := fake(t, 200, `{}`)
	ctx := context.Background()
	_ = c.ArchiveMeter(ctx, "a/b")
	_, _ = c.AssignPlan(ctx, "org_42", PlanRef{Plan: "free", Version: 3})
	_ = c.UnassignPlan(ctx, "org_42")
	_, _ = c.UpdateMeter(ctx, "m", "Name", "Desc")
	want := []seen{
		{method: "POST", path: "/v1/meter/meters/a%2Fb:archive"},
		{method: "POST", path: "/v1/meter/customers/org_42:assignPlan"},
		{method: "POST", path: "/v1/meter/customers/org_42:unassignPlan"},
		{method: "PATCH", path: "/v1/meter/meters/m", query: "updateMask=name,description"},
	}
	for i, w := range want {
		g := (*got)[i]
		if g.method != w.method || g.path != w.path || g.query != w.query {
			t.Errorf("request %d: got %s %s?%s, want %s %s?%s", i, g.method, g.path, g.query, w.method, w.path, w.query)
		}
	}
	if (*got)[1].body != `{"plan":"free","version":3}` {
		t.Errorf("assign body: %s", (*got)[1].body)
	}
}

func TestErrorsCarryCodeReasonAndNotFound(t *testing.T) {
	c, _ := fake(t, 404, `{"code":"not_found","message":"no meter x"}`)
	_, err := c.GetMeter(context.Background(), "x")
	if !IsNotFound(err) {
		t.Fatalf("want not found, got %v", err)
	}
	c, _ = fake(t, 400, `{"code":"failed_precondition","message":"meter has events","details":[{"reason":"meter_semantics_immutable"}]}`)
	_, err = c.CreateMeter(context.Background(), Meter{Slug: "x"})
	if err == nil || IsNotFound(err) || err.Error() != "400 failed_precondition (meter_semantics_immutable): meter has events" {
		t.Fatalf("error: %v", err)
	}
}

func TestNewRefusesBadInput(t *testing.T) {
	if _, err := New("ftp://x", "k", ""); err == nil {
		t.Error("ftp endpoint accepted")
	}
	if _, err := New("https://x", " ", ""); err == nil {
		t.Error("empty key accepted")
	}
}
