import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { createProject, deleteProject, listProjects, renameProject } from "../lib/api";
import { createProjectsCollection } from "../lib/db-projects";
import type { Project } from "../lib/types";
import { queryKeys } from "../hooks/query/keys";

vi.mock("../lib/api", () => ({
  listProjects: vi.fn(),
  createProject: vi.fn(),
  renameProject: vi.fn(),
  deleteProject: vi.fn(),
}));

const mockedListProjects = vi.mocked(listProjects);
const mockedCreateProject = vi.mocked(createProject);
const mockedRenameProject = vi.mocked(renameProject);
const mockedDeleteProject = vi.mocked(deleteProject);

const projects: Project[] = [
  { id: "p1", name: "One" },
  { id: "p2", name: "Two" },
];

describe("projectsCollection", () => {
  let queryClient: QueryClient;

  beforeEach(() => {
    vi.clearAllMocks();
    queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    mockedListProjects.mockResolvedValue({ projects });
  });

  it("syncs GET /api/projects into items keyed by id", async () => {
    const collection = createProjectsCollection(queryClient);
    await collection.preload();
    expect(collection.get("p1")?.name).toBe("One");
    expect(collection.get("p2")?.name).toBe("Two");
  });

  it("rename: update is optimistic and persists via PATCH", async () => {
    mockedRenameProject.mockResolvedValue(true);
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    const tx = collection.update("p1", (draft) => {
      draft.name = "Uno";
    });
    expect(collection.get("p1")?.name).toBe("Uno");
    expect(mockedRenameProject).toHaveBeenCalledWith("p1", "Uno");
    await tx.when("settled");
  });

  it("rename: a failed PATCH rolls the name back", async () => {
    mockedRenameProject.mockResolvedValue(false);
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    const tx = collection.update("p1", (draft) => {
      draft.name = "Uno";
    });
    await expect(tx.when("settled")).rejects.toThrow("Rename failed");
    expect(collection.get("p1")?.name).toBe("One");
  });

  it("delete: the row leaves immediately and DELETE persists", async () => {
    mockedDeleteProject.mockResolvedValue(true);
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    const tx = collection.delete("p2");
    expect(collection.has("p2")).toBe(false);
    expect(mockedDeleteProject).toHaveBeenCalledWith("p2");
    await tx.when("settled");
  });

  it("delete: a failed DELETE restores the row", async () => {
    mockedDeleteProject.mockResolvedValue(false);
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    const tx = collection.delete("p1");
    await expect(tx.when("settled")).rejects.toThrow("Delete failed");
    expect(collection.get("p1")?.name).toBe("One");
  });

  it("create flow: POST first, then converge the query cache (useCreateProject)", async () => {
    mockedCreateProject.mockResolvedValue({ project: { id: "p3", name: "Three" } });
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    // What useCreateProject's onSuccess does — the cache write syncs in.
    const { project } = await createProject("Three");
    queryClient.setQueryData<Project[]>(queryKeys.projects, (old = []) => [
      ...old.filter((p) => p.id !== project.id),
      project,
    ]);
    expect(collection.get("p3")?.name).toBe("Three");
    expect(mockedCreateProject).toHaveBeenCalledWith("Three");
  });

  it("mirrors query-cache updates into the collection", async () => {
    const collection = createProjectsCollection(queryClient);
    await collection.preload();

    queryClient.setQueryData<Project[]>(queryKeys.projects, [{ id: "p9", name: "Nine" }]);
    expect(collection.get("p9")?.name).toBe("Nine");
    expect(collection.has("p1")).toBe(false);
  });
});
