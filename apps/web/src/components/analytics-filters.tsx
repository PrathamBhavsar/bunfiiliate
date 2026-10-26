"use client";

import { useAffiliates, usePrograms } from "@/lib/admin";
import type { Scope } from "@/lib/analytics";
import { Select } from "./ui/input";

/** Program and affiliate pickers shared by analytics and reports. */
export function ScopeSelects({ scope, onChange }: { scope: Scope; onChange: (s: Scope) => void }) {
  const programs = usePrograms().data;
  const affiliates = useAffiliates().data;
  return (
    <>
      <Select aria-label="Program" className="w-auto" value={scope.program_id} onChange={(e) => onChange({ ...scope, program_id: e.target.value })}>
        <option value="">Program: all</option>
        {programs?.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
          </option>
        ))}
      </Select>
      <Select aria-label="Affiliate" className="w-auto" value={scope.affiliate_id} onChange={(e) => onChange({ ...scope, affiliate_id: e.target.value })}>
        <option value="">Affiliate: all</option>
        {affiliates?.map((a) => (
          <option key={a.id} value={a.id}>
            {a.email}
          </option>
        ))}
      </Select>
    </>
  );
}
