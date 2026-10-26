"use client";

import { ResourceView } from "@/components/resource-view";
import { placeholderResource } from "@/components/placeholder";

const config = placeholderResource("Programs", "T31");

export default function Page() {
  return <ResourceView config={config} />;
}
