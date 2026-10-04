import { describe, expect, it } from "vite-plus/test";
import { LOCAL_NODE_ID, setLocalNodeAlias } from "../lib/format";
import { resumeTargets, type ResumeTargetNode } from "../lib/resume";

const local = (agents: string[] = ["devin", "cline"]): ResumeTargetNode => ({
  label: "This machine",
  agents,
});

const node = (id: string, agents: string[] = ["devin", "cline"]): ResumeTargetNode => ({
  node: id,
  label: id,
  agents,
});

describe("resumeTargets", () => {
  it("returns no targets on a single machine — the menu hides", () => {
    expect(resumeTargets([local()], { agent: "devin" })).toEqual([]);
    expect(resumeTargets([], { agent: "devin" })).toEqual([]);
    expect(resumeTargets([local()], undefined)).toEqual([]);
  });

  it("excludes the session's own node+agent pair, keeps the rest", () => {
    const targets = resumeTargets([local(), node("node_thinkpad")], { agent: "devin" });
    expect(targets.map((t) => t.label)).toEqual(["This machine", "node_thinkpad"]);
    // devin@local is the session's own pair — only cline stays on this node.
    expect(targets[0]?.agents).toEqual(["cline"]);
    // The peer offers its full roster — same agent id on another node is a
    // real move.
    expect(targets[1]?.agents).toEqual(["devin", "cline"]);
  });

  it("matches the local pair whether the row's node is undefined or the 'local' key", () => {
    const candidates = [local(["devin"]), node("node_thinkpad")];
    for (const nodeTag of [undefined, LOCAL_NODE_ID]) {
      const targets = resumeTargets(candidates, { node: nodeTag, agent: "devin" });
      expect(targets[0]?.agents).toEqual([]);
      expect(targets[1]?.agents).toEqual(["devin", "cline"]);
    }
  });

  it("matches the local pair via the server-issued node id once the alias lands", () => {
    setLocalNodeAlias("node_self");
    const targets = resumeTargets([local(), node("node_thinkpad")], {
      node: "node_self",
      agent: "devin",
    });
    expect(targets[0]?.agents).toEqual(["cline"]);
    expect(targets[1]?.agents).toEqual(["devin", "cline"]);
  });

  it("a peer-hosted session targets the local machine and other peers", () => {
    const targets = resumeTargets([local(), node("node_a"), node("node_b")], {
      node: "node_b",
      agent: "devin",
    });
    // This machine and the other peer keep full rosters; the session's own
    // node drops only its own agent.
    expect(targets[0]?.agents).toEqual(["devin", "cline"]);
    expect(targets[1]?.agents).toEqual(["devin", "cline"]);
    expect(targets[2]?.agents).toEqual(["cline"]);
  });

  it("a node left with no agents still appears — the menu shows a disabled row", () => {
    const targets = resumeTargets([local(["devin"]), node("node_thinkpad", ["devin"])], {
      agent: "devin",
    });
    expect(targets).toHaveLength(2);
    expect(targets[0]?.agents).toEqual([]);
    expect(targets[1]?.agents).toEqual(["devin"]);
  });
});
