// @vitest-environment happy-dom
/**
 * `GraphWidget` -- the React Flow tile.
 *
 * This is the first component test in this package, and it exists because a
 * React Flow graph fails in ways markup cannot show. A wrong position, a node
 * with no label, an edge naming a node that is not there: none of them throw in
 * a visible place, the tile just renders fewer boxes than the spec asked for.
 * Asserting on the mounted DOM is the only way to see that.
 *
 * Three states are load-bearing and each has a bug that looks identical from
 * outside: loading, error, and loaded-but-empty. The last is the one a
 * screenshot hides -- an empty canvas and a failed render look the same unless
 * the component says which it is.
 *
 * `happy-dom` provides `ResizeObserver` and `DOMMatrixReadOnly`, both of which
 * React Flow needs. The observer is spied on rather than stubbed: a no-op stub
 * would stop measuring the viewport, which is the very thing under test.
 */
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { GraphWidget } from "../../src/components/GraphWidget.js";
import { previewGraph } from "../../src/api.js";
import type { Widget } from "../../src/api";

vi.mock("../../src/api", () => ({ previewGraph: vi.fn() }));

const preview = vi.mocked(previewGraph);

const GRAPH_PAYLOAD = {
  nodes: [
    { id: "web", position: { x: 0, y: 0 }, data: { label: "Web tier" } },
    { id: "api", position: { x: 240, y: 0 }, data: { label: "API tier" } },
  ],
  edges: [{ id: "e1", source: "web", target: "api" }],
  fitView: true,
  pannable: true,
  zoomable: true,
  theme: { nodeBackground: "#f5f5f5" },
};

const WIDGET = {
  id: "w-graph",
  type: "graph",
  graph: {
    nodes: [
      { id: "web", label: "Web tier" },
      { id: "api", label: "API tier" },
    ],
  },
  interactions: [{ on: "click", set: { service: "label" } }],
} as unknown as Widget;

function renderWidget(over: Partial<Parameters<typeof GraphWidget>[0]> = {}) {
  return render(
    <GraphWidget
      widget={WIDGET}
      editMode={false}
      isSelected={false}
      onSelect={() => {}}
      onDelete={() => {}}
      {...over}
    />,
  );
}

afterEach(() => {
  cleanup();
  preview.mockReset();
});

describe("GraphWidget rendering", () => {
  it("mounts one node per compiled node, labelled", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    renderWidget();
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    expect(screen.getByText("API tier")).toBeTruthy();
  });

  it("shows the graph title from the widget spec", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    renderWidget({
      widget: { ...WIDGET, graph: { ...(WIDGET.graph as object), title: "Service Map" } } as Widget,
    });
    await waitFor(() => expect(screen.getByText("Service Map")).toBeTruthy());
  });

  it("asks the server for the compiled graph on mount", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    renderWidget();
    await waitFor(() => expect(preview).toHaveBeenCalledTimes(1));
    expect(preview.mock.calls[0][0]).toBe(WIDGET);
  });

  it("does not crash on a payload the view module has to repair", async () => {
    // No positions, no labels, a dangling edge: exactly what an unvalidated
    // body produces. The tile must still come up.
    preview.mockResolvedValue({
      nodes: [{ id: "a" }, { id: "b" }],
      edges: [{ id: "e", source: "a", target: "ghost" }],
    } as never);
    renderWidget();
    await waitFor(() => expect(screen.getByText("a")).toBeTruthy());
    expect(screen.queryByText(/error/i)).toBeNull();
  });

  it("does not crash on a wholly malformed payload", async () => {
    preview.mockResolvedValue("nonsense" as never);
    renderWidget();
    await waitFor(() => expect(screen.getByText(/no nodes/i)).toBeTruthy());
  });
});

describe("GraphWidget states", () => {
  it("shows a loading state until the graph arrives", async () => {
    let release!: (value: unknown) => void;
    preview.mockReturnValue(
      new Promise((r) => {
        release = r;
      }) as never,
    );
    const { container } = renderWidget();
    expect(container.querySelector(".animate-spin-slow")).toBeTruthy();
    release(GRAPH_PAYLOAD);
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
  });

  it("shows the server's error message when the request fails", async () => {
    preview.mockRejectedValue(new Error("Failed to preview widget: 500"));
    renderWidget();
    await waitFor(() => expect(screen.getByText(/Failed to preview widget/)).toBeTruthy());
  });

  it("shows an error when the request fails with no message", async () => {
    preview.mockRejectedValue(new Error(""));
    renderWidget();
    await waitFor(() => expect(screen.getByText(/failed to load the graph/i)).toBeTruthy());
  });

  it("clears a previous error when a later request succeeds", async () => {
    preview.mockRejectedValueOnce(new Error("boom"));
    const { rerender } = renderWidget();
    await waitFor(() => expect(screen.getByText(/boom/)).toBeTruthy());

    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    rerender(
      <GraphWidget
        widget={{ ...WIDGET, id: "w-graph-2" } as Widget}
        editMode={false}
        isSelected={false}
        onSelect={() => {}}
        onDelete={() => {}}
      />,
    );
    await waitFor(() => expect(screen.queryByText(/boom/)).toBeNull());
    // Inside `waitFor`: the error clears synchronously at the start of the new
    // request, whereas the graph only lands when that request resolves.
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
  });

  it("distinguishes an empty graph from a failed render", async () => {
    // Both look like a blank canvas; only one of them is a correct answer.
    preview.mockResolvedValue({ nodes: [], edges: [] } as never);
    renderWidget();
    await waitFor(() => expect(screen.getByText(/no nodes/i)).toBeTruthy());
    expect(container_has("animate-spin-slow")).toBe(false);
  });
});

/** Present so the empty-state assertion above reads as one line. */
function container_has(selector: string): boolean {
  return document.querySelector(selector) !== null;
}

describe("GraphWidget interaction", () => {
  it("reports the clicked node as a param change", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    const onParamChange = vi.fn();
    renderWidget({ onParamChange });
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());

    screen.getByText("Web tier").click();

    await waitFor(() => expect(onParamChange).toHaveBeenCalledWith("service", "Web tier"));
  });

  it("reports nothing when the widget declares no click interaction", async () => {
    preview.mockResolvedValue({
      nodes: [{ id: "web", position: { x: 0, y: 0 }, data: { label: "Web tier" } }],
      edges: [],
    } as never);
    const onParamChange = vi.fn();
    renderWidget({
      // No `interactions` at all. The payload has to match this widget's own
      // spec, because the mock answers with whatever it was told to.
      widget: { id: "w", type: "graph", graph: { nodes: [{ id: "web" }] } } as Widget,
      onParamChange,
    });
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    screen.getByText("Web tier").click();
    expect(onParamChange).not.toHaveBeenCalled();
  });

  it("does not throw when no onParamChange handler was passed", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    renderWidget({ onParamChange: undefined });
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    expect(() => screen.getByText("Web tier").click()).not.toThrow();
  });

  it("selects the widget on card click in edit mode only", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    const onSelect = vi.fn();

    // Read mode: the click must reach the graph, not select the tile.
    const readOnly = renderWidget({ onSelect });
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    screen.getByText("Web tier").click();
    expect(onSelect).not.toHaveBeenCalled();
    readOnly.unmount();

    // Edit mode: the same click selects.
    renderWidget({ onSelect, editMode: true });
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    screen.getByText("Web tier").click();
    expect(onSelect).toHaveBeenCalledWith(WIDGET);
  });

  it("deletes the widget from the header action", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    const onDelete = vi.fn();
    renderWidget({ onDelete, editMode: true });
    const remove = await screen.findByTitle("Delete Widget");
    remove.click();
    expect(onDelete).toHaveBeenCalledWith("w-graph");
  });
});

describe("GraphWidget teardown", () => {
  it("observes its container for resize and stops on unmount", async () => {
    preview.mockResolvedValue(GRAPH_PAYLOAD as never);
    const observe = vi.spyOn(ResizeObserver.prototype, "observe");
    const disconnect = vi.spyOn(ResizeObserver.prototype, "disconnect");

    const { unmount } = renderWidget();
    await waitFor(() => expect(screen.getByText("Web tier")).toBeTruthy());
    expect(observe).toHaveBeenCalled();

    const before = disconnect.mock.calls.length;
    unmount();
    expect(disconnect.mock.calls.length).toBeGreaterThan(before);

    observe.mockRestore();
    disconnect.mockRestore();
  });
});
