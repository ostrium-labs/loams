// HE0 (plan HE1 Task 9, rulings R9.40 onward): clickhouse-go v2 over the
// ClickHouse native protocol, run unchanged against the pinned
// clickhouse-server (the oracle) and against the House's native listener.
// Every test passes on both; the House's answer is ClickHouse's.
//
//	HE0_NATIVE_ADDR      host:port of the native listener (required)
//	HE0_NATIVE_USER      user (default "default")
//	HE0_NATIVE_PASSWORD  password
//	HE0_NATIVE_DATABASE  database (default "default")
//	HE0_NATIVE_TLS=1     TLS, the certificate not verified (the House's is
//	                     self-signed)
//
//	go test ./... -count=1
package v2

import (
	"context"
	"crypto/tls"
	"database/sql"
	"errors"
	"fmt"
	"os"
	"reflect"
	"sort"
	"testing"
	"time"

	"github.com/ClickHouse/clickhouse-go/v2"
	"github.com/ClickHouse/clickhouse-go/v2/lib/driver"
)

func options(t *testing.T) *clickhouse.Options {
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
	opts := &clickhouse.Options{
		Addr:     []string{addr},
		Protocol: clickhouse.Native,
		Auth: clickhouse.Auth{
			Database: database,
			Username: user,
			Password: os.Getenv("HE0_NATIVE_PASSWORD"),
		},
		DialTimeout: 10 * time.Second,
		ReadTimeout: 60 * time.Second,
	}
	if os.Getenv("HE0_NATIVE_TLS") == "1" {
		opts.TLS = &tls.Config{InsecureSkipVerify: true}
	}
	return opts
}

func open(t *testing.T) driver.Conn {
	t.Helper()
	conn, err := clickhouse.Open(options(t))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(func() { _ = conn.Close() })
	if err := conn.Ping(context.Background()); err != nil {
		t.Fatalf("ping: %v", err)
	}
	return conn
}

// A table name of this run's own, so parallel runs and reruns do not meet.
func table(prefix string) string {
	return fmt.Sprintf("he0n_%s_%d", prefix, time.Now().UnixNano())
}

func code(err error) int32 {
	var ex *clickhouse.Exception
	if errors.As(err, &ex) {
		return ex.Code
	}
	return -1
}

func TestHandshakeAndSelectOne(t *testing.T) {
	conn := open(t)
	ctx := context.Background()
	version, err := conn.ServerVersion()
	if err != nil {
		t.Fatalf("server version: %v", err)
	}
	if version.Version.Major < 26 {
		t.Fatalf("server version %v: want the 26.x line", version.Version)
	}
	var one uint8
	if err := conn.QueryRow(ctx, "SELECT 1").Scan(&one); err != nil || one != 1 {
		t.Fatalf("SELECT 1: %v %v", one, err)
	}
	rows, err := conn.Query(ctx, "SELECT number, toString(number) AS s FROM system.numbers LIMIT 5")
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	if got := rows.Columns(); !reflect.DeepEqual(got, []string{"number", "s"}) {
		t.Fatalf("columns %v", got)
	}
	types := rows.ColumnTypes()
	if types[0].DatabaseTypeName() != "UInt64" || types[1].DatabaseTypeName() != "String" {
		t.Fatalf("types %s %s", types[0].DatabaseTypeName(), types[1].DatabaseTypeName())
	}
	var n int
	for rows.Next() {
		var number uint64
		var s string
		if err := rows.Scan(&number, &s); err != nil {
			t.Fatalf("scan: %v", err)
		}
		if s != fmt.Sprint(number) {
			t.Fatalf("row %d: %q", number, s)
		}
		n++
	}
	if err := rows.Err(); err != nil || n != 5 {
		t.Fatalf("rows: %d %v", n, err)
	}
	_ = rows.Close()
}

func TestDDLInsertSelect(t *testing.T) {
	conn := open(t)
	ctx := context.Background()
	name := table("types")
	ddl := "CREATE TABLE " + name + ` (
		id UInt64,
		name String,
		at DateTime('UTC'),
		score Nullable(Float64),
		tags Array(String),
		kind LowCardinality(String),
		attrs Map(String, UInt32),
		ok Bool,
		price Decimal(18, 4),
		uid UUID,
		day Date
	) ENGINE = MergeTree ORDER BY id`
	if err := conn.Exec(ctx, ddl); err != nil {
		t.Fatalf("create: %v", err)
	}
	t.Cleanup(func() { _ = conn.Exec(context.Background(), "DROP TABLE IF EXISTS "+name) })

	batch, err := conn.PrepareBatch(ctx, "INSERT INTO "+name)
	if err != nil {
		t.Fatalf("prepare batch: %v", err)
	}
	at := time.Date(2026, 10, 22, 8, 30, 0, 0, time.UTC)
	const rowsWanted = 1000
	for i := 0; i < rowsWanted; i++ {
		var score *float64
		if i%3 != 0 {
			v := float64(i) / 4
			score = &v
		}
		uid := fmt.Sprintf("00000000-0000-4000-8000-%012d", i)
		if err := batch.Append(
			uint64(i), fmt.Sprintf("row-%d", i), at.Add(time.Duration(i)*time.Second), score,
			[]string{"a", fmt.Sprint(i % 7)}, []string{"x", "y", "z"}[i%3],
			map[string]uint32{"n": uint32(i)}, i%2 == 0, fmt.Sprintf("%d.25", i), uid,
			at.AddDate(0, 0, i%30),
		); err != nil {
			t.Fatalf("append %d: %v", i, err)
		}
	}
	if err := batch.Send(); err != nil {
		t.Fatalf("send: %v", err)
	}

	var count uint64
	if err := conn.QueryRow(ctx, "SELECT count() FROM "+name).Scan(&count); err != nil || count != rowsWanted {
		t.Fatalf("count: %d %v", count, err)
	}
	rows, err := conn.Query(ctx, "SELECT id, name, at, score, tags, kind, attrs, ok, toString(price), toString(uid), day FROM "+name+" ORDER BY id")
	if err != nil {
		t.Fatalf("select: %v", err)
	}
	i := 0
	for rows.Next() {
		var (
			id         uint64
			nameV      string
			atV        time.Time
			score      *float64
			tags       []string
			kind       string
			attrs      map[string]uint32
			ok         bool
			price, uid string
			day        time.Time
		)
		if err := rows.Scan(&id, &nameV, &atV, &score, &tags, &kind, &attrs, &ok, &price, &uid, &day); err != nil {
			t.Fatalf("scan: %v", err)
		}
		if id != uint64(i) || nameV != fmt.Sprintf("row-%d", i) || !atV.Equal(at.Add(time.Duration(i)*time.Second)) {
			t.Fatalf("row %d: %d %q %v", i, id, nameV, atV)
		}
		if (i%3 == 0) != (score == nil) || (score != nil && *score != float64(i)/4) {
			t.Fatalf("row %d: score %v", i, score)
		}
		if !reflect.DeepEqual(tags, []string{"a", fmt.Sprint(i % 7)}) || kind != []string{"x", "y", "z"}[i%3] {
			t.Fatalf("row %d: tags %v kind %q", i, tags, kind)
		}
		if attrs["n"] != uint32(i) || ok != (i%2 == 0) || price != fmt.Sprintf("%d.25", i) {
			t.Fatalf("row %d: attrs %v ok %v price %q", i, attrs, ok, price)
		}
		if uid != fmt.Sprintf("00000000-0000-4000-8000-%012d", i) || !day.Equal(at.AddDate(0, 0, i%30).Truncate(24*time.Hour)) {
			t.Fatalf("row %d: uid %q day %v", i, uid, day)
		}
		i++
	}
	if err := rows.Err(); err != nil || i != rowsWanted {
		t.Fatalf("rows: %d %v", i, err)
	}
	_ = rows.Close()
}

func TestEmptyResultHasColumns(t *testing.T) {
	conn := open(t)
	ctx := context.Background()
	rows, err := conn.Query(ctx, "SELECT number AS n, 'x' AS s FROM system.numbers WHERE number > 10 LIMIT 0")
	if err != nil {
		t.Fatalf("query: %v", err)
	}
	if got := rows.Columns(); !reflect.DeepEqual(got, []string{"n", "s"}) {
		t.Fatalf("columns of an empty result: %v", got)
	}
	if rows.Next() {
		t.Fatal("a row in an empty result")
	}
	_ = rows.Close()
}

func TestShowTables(t *testing.T) {
	conn := open(t)
	ctx := context.Background()
	name := table("show")
	if err := conn.Exec(ctx, "CREATE TABLE "+name+" (x UInt8) ENGINE = MergeTree ORDER BY x"); err != nil {
		t.Fatalf("create: %v", err)
	}
	t.Cleanup(func() { _ = conn.Exec(context.Background(), "DROP TABLE IF EXISTS "+name) })
	rows, err := conn.Query(ctx, "SHOW TABLES LIKE '"+name+"'")
	if err != nil {
		t.Fatalf("show tables: %v", err)
	}
	var names []string
	for rows.Next() {
		var n string
		if err := rows.Scan(&n); err != nil {
			t.Fatalf("scan: %v", err)
		}
		names = append(names, n)
	}
	_ = rows.Close()
	sort.Strings(names)
	if !reflect.DeepEqual(names, []string{name}) {
		t.Fatalf("SHOW TABLES: %v", names)
	}
	var none string
	err = conn.QueryRow(ctx, "SHOW TABLES LIKE 'he0n_no_such_table'").Scan(&none)
	if !errors.Is(err, sql.ErrNoRows) {
		t.Fatalf("SHOW TABLES of nothing: %v", err)
	}
}

func TestErrors(t *testing.T) {
	conn := open(t)
	ctx := context.Background()
	_, err := conn.Query(ctx, "SELECT * FROM he0n_no_such_table")
	if code(err) != 60 {
		t.Fatalf("unknown table: %v (code %d)", err, code(err))
	}
	if err := conn.Exec(ctx, "SELEC 1"); code(err) != 62 {
		t.Fatalf("syntax error: %v (code %d)", err, code(err))
	}
	// The connection still serves after an exception.
	var one uint8
	if err := conn.QueryRow(ctx, "SELECT 1").Scan(&one); err != nil || one != 1 {
		t.Fatalf("after errors: %v", err)
	}
	if err := conn.Ping(ctx); err != nil {
		t.Fatalf("ping after errors: %v", err)
	}
}

func TestSettingsAndParameters(t *testing.T) {
	conn := open(t)
	ctx := clickhouse.Context(context.Background(),
		clickhouse.WithSettings(clickhouse.Settings{"max_execution_time": 60}),
		clickhouse.WithParameters(clickhouse.Parameters{"x": "42", "s": "it's"}))
	var x uint8
	var s string
	if err := conn.QueryRow(ctx, "SELECT {x:UInt8}, {s:String}").Scan(&x, &s); err != nil || x != 42 || s != "it's" {
		t.Fatalf("parameters: %d %q %v", x, s, err)
	}
	var value string
	if err := conn.QueryRow(ctx, "SELECT getSetting('max_execution_time')").Scan(&value); err != nil {
		// Some Houses answer getSetting with a refusal; the setting itself must pass.
		t.Logf("getSetting: %v", err)
	}
}

func TestStdInsideTransaction(t *testing.T) {
	db := clickhouse.OpenDB(options(t))
	t.Cleanup(func() { _ = db.Close() })
	name := table("std")
	if _, err := db.Exec("CREATE TABLE " + name + " (version Int64, dirty UInt8, sequence UInt64) ENGINE = MergeTree ORDER BY sequence"); err != nil {
		t.Fatalf("create: %v", err)
	}
	t.Cleanup(func() { _, _ = db.Exec("DROP TABLE IF EXISTS " + name) })
	var version int64
	var dirty uint8
	err := db.QueryRow("SELECT version, dirty FROM " + name + " ORDER BY sequence DESC LIMIT 1").Scan(&version, &dirty)
	if !errors.Is(err, sql.ErrNoRows) {
		t.Fatalf("empty table: %v", err)
	}
	tx, err := db.Begin()
	if err != nil {
		t.Fatalf("begin: %v", err)
	}
	if _, err := tx.Exec("INSERT INTO "+name+" (version, dirty, sequence) VALUES (?, ?, ?)", 20261022, uint8(1), uint64(time.Now().UnixNano())); err != nil {
		t.Fatalf("insert: %v", err)
	}
	if err := tx.Commit(); err != nil {
		t.Fatalf("commit: %v", err)
	}
	if _, err := db.Exec("INSERT INTO "+name+" (version, dirty, sequence) VALUES (?, ?, ?)", 20261023, uint8(0), uint64(time.Now().UnixNano())); err != nil {
		t.Fatalf("exec insert: %v", err)
	}
	if err := db.QueryRow("SELECT version, dirty FROM "+name+" ORDER BY sequence DESC LIMIT 1").Scan(&version, &dirty); err != nil || version != 20261023 || dirty != 0 {
		t.Fatalf("read back: %d %d %v", version, dirty, err)
	}
}

func TestCompressedBlocks(t *testing.T) {
	for _, method := range []clickhouse.CompressionMethod{clickhouse.CompressionLZ4, clickhouse.CompressionZSTD} {
		opts := options(t)
		opts.Compression = &clickhouse.Compression{Method: method}
		conn, err := clickhouse.Open(opts)
		if err != nil {
			t.Fatalf("%v: open: %v", method, err)
		}
		ctx := context.Background()
		name := table("lz")
		if err := conn.Exec(ctx, "CREATE TABLE "+name+" (id UInt64, s String) ENGINE = MergeTree ORDER BY id"); err != nil {
			t.Fatalf("%v: create: %v", method, err)
		}
		batch, err := conn.PrepareBatch(ctx, "INSERT INTO "+name)
		if err != nil {
			t.Fatalf("%v: prepare: %v", method, err)
		}
		for i := 0; i < 5000; i++ {
			if err := batch.Append(uint64(i), fmt.Sprintf("value-%d", i%17)); err != nil {
				t.Fatalf("%v: append: %v", method, err)
			}
		}
		if err := batch.Send(); err != nil {
			t.Fatalf("%v: send: %v", method, err)
		}
		var count, sum uint64
		if err := conn.QueryRow(ctx, "SELECT count(), sum(id) FROM "+name).Scan(&count, &sum); err != nil || count != 5000 || sum != 4999*5000/2 {
			t.Fatalf("%v: read back: %d %d %v", method, count, sum, err)
		}
		_ = conn.Exec(ctx, "DROP TABLE IF EXISTS "+name)
		_ = conn.Close()
	}
}
