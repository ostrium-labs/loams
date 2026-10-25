package migrate

// golang-migrate's ClickHouse driver, as Langfuse runs it (`migrate -database
// clickhouse://…?x-multi-statement=true&x-migrations-table-engine=MergeTree up`).
// That driver is built on clickhouse-go v1 (protocol revision 54213), not v2:
// the House's native listener serves both.

import (
	"database/sql"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"testing"
	"time"

	_ "github.com/ClickHouse/clickhouse-go" // v1, the driver golang-migrate links
	"github.com/golang-migrate/migrate/v4"
	_ "github.com/golang-migrate/migrate/v4/database/clickhouse"
	_ "github.com/golang-migrate/migrate/v4/source/file"
)

func v1DSN(t *testing.T, extra string) string {
	t.Helper()
	addr := os.Getenv("HE0_NATIVE_ADDR")
	if addr == "" {
		t.Skip("HE0_NATIVE_ADDR is not set")
	}
	user := os.Getenv("HE0_NATIVE_USER")
	if user == "" {
		user = "default"
	}
	database := os.Getenv("HE0_NATIVE_DATABASE")
	if database == "" {
		database = "default"
	}
	dsn := fmt.Sprintf("clickhouse://%s?username=%s&password=%s&database=%s", addr,
		url.QueryEscape(user), url.QueryEscape(os.Getenv("HE0_NATIVE_PASSWORD")), database)
	if os.Getenv("HE0_NATIVE_TLS") == "1" {
		dsn += "&secure=true&skip_verify=true"
	}
	return dsn + extra
}

func TestGolangMigrateUp(t *testing.T) {
	stamp := time.Now().UnixNano()
	events := fmt.Sprintf("he0n_events_%d", stamp)
	scores := fmt.Sprintf("he0n_scores_%d", stamp)
	migrations := fmt.Sprintf("he0n_migrations_%d", stamp)
	dir := t.TempDir()
	files := map[string]string{
		"0001_events.up.sql": "CREATE TABLE " + events + ` (
			id String, project_id String, ts DateTime64(3), name String, value Float64,
			tags Array(String), is_deleted UInt8, event_ts DateTime64(3)
		) ENGINE = ReplacingMergeTree(event_ts, is_deleted)
		ORDER BY (project_id, toDate(ts), id);`,
		"0001_events.down.sql": "DROP TABLE IF EXISTS " + events + ";",
		"0002_scores.up.sql": "CREATE TABLE " + scores + ` (id String, score Float64) ENGINE = MergeTree ORDER BY id;
			CREATE TABLE ` + scores + `_by_name (name String, score Float64) ENGINE = MergeTree ORDER BY name;`,
		"0002_scores.down.sql": "DROP TABLE IF EXISTS " + scores + "; DROP TABLE IF EXISTS " + scores + "_by_name;",
	}
	for name, body := range files {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	dsn := v1DSN(t, "&x-multi-statement=true&x-migrations-table-engine=MergeTree&x-migrations-table="+migrations)
	m, err := migrate.New("file://"+dir, dsn)
	if err != nil {
		t.Fatalf("migrate.New: %v", err)
	}
	t.Cleanup(func() {
		db, err := sql.Open("clickhouse", v1DSN(t, ""))
		if err == nil {
			for _, name := range []string{events, scores, scores + "_by_name", migrations} {
				_, _ = db.Exec("DROP TABLE IF EXISTS " + name)
			}
			_ = db.Close()
		}
	})
	if err := m.Up(); err != nil {
		t.Fatalf("up: %v", err)
	}
	version, dirty, err := m.Version()
	if err != nil || version != 2 || dirty {
		t.Fatalf("version %d dirty %v: %v", version, dirty, err)
	}
	// A second run finds nothing to do, as Langfuse's restarts do.
	if err := m.Up(); err != migrate.ErrNoChange {
		t.Fatalf("second up: %v", err)
	}
	srcErr, dbErr := m.Close()
	if srcErr != nil || dbErr != nil {
		t.Fatalf("close: %v %v", srcErr, dbErr)
	}

	db, err := sql.Open("clickhouse", v1DSN(t, ""))
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	// golang-migrate's driver refuses INSERT outside a transaction, so the
	// migrations hold DDL only, as Langfuse's do; the tables are there and empty.
	var count uint64
	if err := db.QueryRow("SELECT count() FROM " + scores + "_by_name").Scan(&count); err != nil || count != 0 {
		t.Fatalf("scores: %v %v", count, err)
	}
}
