"use client";

import { ActionButton } from "@/components/action-button";
import { ResourceView, type ResourceConfig } from "@/components/resource-view";
import { StatusBadge } from "@/components/status-badge";
import { formatTime, ok, refresh, toPage } from "@/lib/admin";
import { api } from "@/lib/api";
import type { components } from "@bunfiiliate/sdk";
import type { ListFn } from "@/lib/use-resource";

type Notification = components["schemas"]["Notification"];

const list: ListFn<Notification> = async ({ page, pageSize, filters }, signal) =>
  toPage(
    await ok(
      api.GET("/api/v1/notifications", {
        signal,
        params: { query: { page, limit: pageSize, unread: filters.unread === "true" } },
      }),
    ),
  );

const config: ResourceConfig<Notification> = {
  title: "Notifications",
  description: "Events about your account, newest first.",
  list,
  filters: [{ id: "unread", label: "Show", options: [{ value: "true", label: "Unread only" }] }],
  columns: [
    { id: "body", header: "Message", className: "max-w-[28rem] truncate", cell: (n) => <span className={n.read_at ? "text-muted" : "font-medium"}>{n.body}</span> },
    { id: "kind", header: "Kind", cell: (n) => n.kind },
    { id: "at", header: "Received", cell: (n) => formatTime(n.created_at) },
    {
      id: "state",
      header: "Status",
      cell: (n) =>
        n.read_at ? (
          <StatusBadge status="read" />
        ) : (
          <ActionButton
            size="sm"
            onAction={async () => {
              await ok(api.POST("/api/v1/notifications/{id}/read", { params: { path: { id: n.id } } }));
              await refresh();
            }}
          >
            Mark read
          </ActionButton>
        ),
    },
  ],
  empty: { title: "No notifications", description: "You will see approvals and payouts here as they happen." },
};

export default function Page() {
  return <ResourceView config={config} />;
}
