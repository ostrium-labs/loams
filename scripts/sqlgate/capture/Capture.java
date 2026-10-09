// Connector/J client for the gate codec fixtures (SQ1 Task 3).
// Run: java -cp mysql-connector-j-<v>.jar Capture.java <port> [sslMode]
import java.sql.*;

public class Capture {
    public static void main(String[] a) throws Exception {
        String url = "jdbc:mysql://127.0.0.1:" + a[0] + "/?user=loams_cap&password=capture"
            + "&allowPublicKeyRetrieval=true&connectTimeout=5000"
            + "&sslMode=" + (a.length > 1 ? a[1] : "PREFERRED");
        try (Connection c = DriverManager.getConnection(url);
             ResultSet r = c.createStatement().executeQuery("SELECT 1")) {
            r.next();
            System.out.println("connector/j: " + r.getInt(1));
        } catch (SQLException e) {
            System.out.println("connector/j: " + e.getMessage());
        }
    }
}
