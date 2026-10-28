// The Loams cases' app for DBOS Transact Java (DX0 Task 5), with the same
// commands as ../../py/app.py: setup, start <wf> (CRASH_AT kills it when that
// step starts), resume <wf>, count <wf>, enqueue <prefix> <n>, serve <s>,
// queued <prefix>, set-event <wf> <v> [s], get-event <wf> <s>.
// Environment: DBOS_URL (postgresql://user:password@host:port/db), DBOS_LISTEN, DBOS_EXECUTOR.
package loams;

import dev.dbos.transact.DBOS;
import dev.dbos.transact.StartWorkflowOptions;
import dev.dbos.transact.config.DBOSConfig;
import dev.dbos.transact.workflow.Queue;
import dev.dbos.transact.workflow.Workflow;
import java.net.URI;
import java.sql.Connection;
import java.sql.DriverManager;
import java.sql.ResultSet;
import java.time.Duration;
import java.util.Map;
import java.util.TreeMap;

public final class App {
  interface Flows {
    String five(String wf) throws Exception;

    String one(String wf) throws Exception;

    String setter(String v) throws Exception;
  }

  static String jdbc;
  static String user;
  static String password;
  static int crashAt = Integer.parseInt(System.getenv().getOrDefault("CRASH_AT", "0"));
  static DBOS dbos;

  static Connection conn() throws Exception {
    return DriverManager.getConnection(jdbc, user, password);
  }

  static void exec(String q, Object... args) throws Exception {
    try (var c = conn(); var st = c.prepareStatement(q)) {
      for (int i = 0; i < args.length; i++) st.setObject(i + 1, args[i]);
      st.execute();
    }
  }

  static final class FlowsImpl implements Flows {
    private void step(String wf, int n) throws Exception {
      dbos.runStep(
          () -> {
            if (n == crashAt) Runtime.getRuntime().halt(137);
            exec("INSERT INTO loams_steps (wf, step) VALUES (?, ?)", wf, n);
            return n;
          },
          "step" + n);
    }

    @Workflow(name = "five")
    public String five(String wf) throws Exception {
      for (int n = 1; n <= 5; n++) step(wf, n);
      return "done";
    }

    @Workflow(name = "one")
    public String one(String wf) throws Exception {
      step(wf, 1);
      return wf;
    }

    @Workflow(name = "setter")
    public String setter(String v) throws Exception {
      dbos.setEvent("k", v);
      dbos.recv("done", Duration.ofMinutes(1));
      return v;
    }
  }

  public static void main(String[] args) throws Exception {
    URI u = URI.create(System.getenv("DBOS_URL"));
    String[] userInfo = u.getUserInfo().split(":", 2);
    user = userInfo[0];
    password = userInfo.length > 1 ? userInfo[1] : "";
    jdbc = "jdbc:postgresql://" + u.getHost() + ":" + u.getPort() + u.getPath();
    String cmd = args[0];
    switch (cmd) {
      case "setup" -> {
        exec("CREATE TABLE IF NOT EXISTS loams_steps (wf text NOT NULL, step int NOT NULL)");
        return;
      }
      case "count" -> {
        Map<String, Integer> m = new TreeMap<>();
        try (var c = conn();
            var st = c.prepareStatement("SELECT step, count(*)::int FROM loams_steps WHERE wf = ? GROUP BY step")) {
          st.setString(1, args[1]);
          try (ResultSet rs = st.executeQuery()) {
            while (rs.next()) m.put(String.valueOf(rs.getInt(1)), rs.getInt(2));
          }
        }
        StringBuilder sb = new StringBuilder("{");
        m.forEach((k, v) -> sb.append(sb.length() > 1 ? ", " : "").append('"').append(k).append("\": ").append(v));
        System.out.println(sb.append('}'));
        return;
      }
      case "queued" -> {
        try (var c = conn();
            var st = c.prepareStatement(
                "SELECT count(*)::int, coalesce(max(n), 0)::int FROM (SELECT count(*) AS n FROM loams_steps WHERE wf LIKE ? GROUP BY wf) t")) {
          st.setString(1, args[1] + "%");
          try (ResultSet rs = st.executeQuery()) {
            rs.next();
            System.out.printf("{\"workflows\": %d, \"max_runs\": %d}%n", rs.getInt(1), rs.getInt(2));
          }
        }
        return;
      }
      default -> {}
    }
    var config = DBOSConfig.defaults("loams-crash-java")
        .withDatabaseUrl(jdbc)
        .withDbUser(user)
        .withDbPassword(password)
        .withUseListenNotify(!"0".equals(System.getenv().getOrDefault("DBOS_LISTEN", "1")))
        .withExecutorId(System.getenv().getOrDefault("DBOS_EXECUTOR", "local"));
    dbos = new DBOS(config);
    Flows flows = dbos.registerProxy(Flows.class, new FlowsImpl());
    Queue queue = new Queue("loams").withConcurrency(10);
    dbos.registerQueue(queue);
    dbos.launch();
    try {
      switch (cmd) {
        case "start" -> System.out.println(
            dbos.startWorkflow(() -> flows.five(args[1]), new StartWorkflowOptions(args[1])).getResult());
        case "resume" -> System.out.println(dbos.retrieveWorkflow(args[1]).getResult());
        case "enqueue" -> {
          for (int i = 0; i < Integer.parseInt(args[2]); i++) {
            String wf = String.format("%s-%05d", args[1], i);
            dbos.startWorkflow(() -> flows.one(wf), new StartWorkflowOptions(wf).withQueue(queue));
          }
          System.out.println("enqueued");
        }
        case "serve" -> Thread.sleep(Long.parseLong(args[1]) * 1000);
        case "set-event" -> {
          var h = dbos.startWorkflow(() -> flows.setter(args[2]), new StartWorkflowOptions(args[1]));
          System.out.printf("{\"set_at\": %.6f}%n", System.currentTimeMillis() / 1000.0);
          System.out.flush();
          Thread.sleep((args.length > 3 ? Long.parseLong(args[3]) : 5) * 1000);
          dbos.send(args[1], "ok", "done");
          h.getResult();
        }
        case "get-event" -> {
          var v = dbos.getEvent(args[1], "k", Duration.ofSeconds(Long.parseLong(args[2])));
          System.out.printf(
              "{\"value\": \"%s\", \"got_at\": %.6f}%n", v.orElse(""), System.currentTimeMillis() / 1000.0);
        }
        default -> throw new IllegalArgumentException("unknown command " + cmd);
      }
    } finally {
      dbos.shutdown();
    }
  }
}
