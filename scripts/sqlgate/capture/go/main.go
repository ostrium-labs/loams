// go-sql-driver client for the gate codec fixtures (SQ1 Task 3). Run by
// capture.sh inside the pinned golang image: go run . <port>
package main

import (
	"database/sql"
	"fmt"
	"os"

	_ "github.com/go-sql-driver/mysql"
)

func main() {
	dsn := "loams_cap:capture@tcp(127.0.0.1:" + os.Args[1] + ")/?timeout=5s&allowPublicKeyRetrieval=true"
	db, err := sql.Open("mysql", dsn)
	if err != nil {
		fmt.Println("go-sql-driver:", err)
		return
	}
	db.SetMaxOpenConns(1)
	var one int
	err = db.QueryRow("SELECT 1").Scan(&one)
	fmt.Println("go-sql-driver:", one, err)
}
