package dev.loams.conformance.temporal.corpus;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.temporal.workflow.WorkflowMethod;
import java.lang.reflect.Method;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import org.junit.jupiter.api.Test;

/** The Java worker's scenario list against scenarios.json, and the generator's flag parsing. */
class ScenariosTest {
  @Test
  void lists_scenarios_json_in_order() throws Exception {
    JsonNode doc = new ObjectMapper().readTree(Path.of("../../scenarios.json").toFile());
    List<String> want = new ArrayList<>();
    List<String> phases = new ArrayList<>();
    for (JsonNode s : doc.get("scenarios")) {
      want.add(s.get("name").asText());
      phases.add(s.get("phase").asText());
    }
    assertEquals(want, Scenarios.ALL.stream().map(Scenarios.Scenario::name).toList());
    assertEquals(phases, Scenarios.ALL.stream().map(Scenarios.Scenario::phase).toList());
  }

  @Test
  void every_scenario_has_a_workflow_of_its_name() {
    Set<String> types = new HashSet<>();
    for (Class<?> impl : Workflows.IMPLEMENTATIONS) {
      for (Class<?> iface : impl.getInterfaces()) {
        for (Method m : iface.getMethods()) {
          WorkflowMethod wm = m.getAnnotation(WorkflowMethod.class);
          if (wm != null) {
            assertTrue(types.add(wm.name()), "two workflows named " + wm.name());
          }
        }
      }
    }
    for (Scenarios.Scenario s : Scenarios.ALL) {
      assertTrue(types.contains(s.name()), "no workflow for " + s.name());
    }
    assertTrue(types.contains("child_greeter"));
    assertTrue(types.contains("child_sleeper"));
  }

  @Test
  void scenario_of_a_history_file() {
    assertEquals("hello", Scenarios.scenarioOf("hello.json"));
    assertEquals("continue_as_new_3", Scenarios.scenarioOf("continue_as_new_3.run2.json"));
  }

  @Test
  void parses_flags_and_picks_in_plan_order() {
    Generate.Args a =
        Generate.Args.parse(
            new String[] {
              "generate", "--address", "127.0.0.1:7233", "--namespace", "default",
              "--out", "/out/java", "--scenarios", "signal_then_complete,hello",
              "--no-search-attribute"
            });
    assertEquals("generate", a.command());
    assertEquals("127.0.0.1:7233", a.address());
    assertFalse(a.searchAttribute());
    assertEquals(List.of("hello", "signal_then_complete"), a.pick());
    assertEquals(Scenarios.ALL.size(), Generate.Args.parse(new String[] {"generate"}).pick().size());
    assertThrows(
        IllegalArgumentException.class,
        () -> Generate.Args.parse(new String[] {"generate", "--scenarios", "nope"}).pick());
  }
}
