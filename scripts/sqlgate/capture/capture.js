// mysql2 client for the gate codec fixtures (SQ1 Task 3). NODE_PATH must
// point at a node_modules directory that holds mysql2.
const mysql = require('mysql2');
const c = mysql.createConnection({
  host: '127.0.0.1', port: Number(process.argv[2]), user: 'loams_cap', password: 'capture',
  connectTimeout: 5000,
});
c.query('SELECT 1 AS one', (err, rows) => {
  console.log('mysql2:', err ? err.message : JSON.stringify(rows));
  c.destroy();
});
