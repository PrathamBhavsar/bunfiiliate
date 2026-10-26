"use client";

import { ResourceView } from "@/components/resource-view";
import { placeholderResource } from "@/components/placeholder";

const config = placeholderResource("Customers", "T32");

export default function Page() {
  return <ResourceView config={config} />;
}
