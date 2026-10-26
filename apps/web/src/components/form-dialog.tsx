"use client";

import { useId, useState, type FormEvent, type ReactNode } from "react";
import { errorMessage } from "@/lib/api";
import { Button } from "./ui/button";
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from "./ui/dialog";
import { Input, Label, Select, Textarea } from "./ui/input";

export type Field = {
  name: string;
  label: string;
  type?: "text" | "email" | "password" | "number" | "textarea" | "select";
  options?: { value: string; label: string }[];
  required?: boolean;
  placeholder?: string;
  hint?: string;
};

export type FormValues = Record<string, string>;

/** Modal form driven by a field list. Values arrive as strings; the caller parses them (cents as integers). */
export function FormDialog({
  trigger,
  title,
  description,
  fields,
  initial = {},
  submitLabel = "Save",
  onSubmit,
}: {
  trigger: ReactNode;
  title: string;
  description?: string;
  fields: Field[];
  initial?: FormValues;
  submitLabel?: string;
  onSubmit: (values: FormValues) => Promise<unknown>;
}) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const values: FormValues = {};
    for (const [k, v] of new FormData(e.currentTarget)) if (typeof v === "string") values[k] = v;
    setPending(true);
    setError(null);
    try {
      await onSubmit(values);
      setOpen(false);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        setError(null);
      }}
    >
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent>
        <div className="grid gap-1">
          <DialogTitle>{title}</DialogTitle>
          {description && <DialogDescription>{description}</DialogDescription>}
        </div>
        <form onSubmit={submit} className="grid gap-3">
          {fields.map((f) => {
            const fid = `${id}-${f.name}`;
            const common = {
              id: fid,
              name: f.name,
              required: f.required,
              defaultValue: initial[f.name] ?? "",
              "aria-describedby": f.hint ? `${fid}-hint` : undefined,
            };
            return (
              <div key={f.name} className="grid gap-1">
                <Label htmlFor={fid}>{f.label}</Label>
                {f.type === "textarea" ? (
                  <Textarea {...common} placeholder={f.placeholder} />
                ) : f.type === "select" ? (
                  <Select {...common}>
                    {f.options?.map((o) => (
                      <option key={o.value} value={o.value}>
                        {o.label}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input {...common} type={f.type ?? "text"} placeholder={f.placeholder} />
                )}
                {f.hint && (
                  <p id={`${fid}-hint`} className="text-xs text-muted">
                    {f.hint}
                  </p>
                )}
              </div>
            );
          })}
          {error && (
            <p role="alert" className="text-xs text-danger">
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <DialogClose asChild>
              <Button>Cancel</Button>
            </DialogClose>
            <Button type="submit" variant="primary" disabled={pending}>
              {pending ? "Saving…" : submitLabel}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  );
}
