import type { ResourceConfig } from "./resource-view";

const noRows = async () => ({ items: [], total: 0 });

/** Stub config for nav pages whose API and table arrive in a later task. */
export function placeholderResource(title: string, task: string): ResourceConfig<{ id: string }> {
  return {
    title,
    description: "Placeholder page.",
    placeholder: task,
    columns: [{ id: "id", header: "ID", cell: (row) => row.id }],
    list: noRows,
    empty: {
      title: `No ${title.toLowerCase()} to show`,
      description: `This page is a placeholder. The ${title.toLowerCase()} API and table are wired in ${task}.`,
    },
  };
}
