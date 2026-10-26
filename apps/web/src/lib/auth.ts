"use client";

import { useMutation, useQuery } from "@tanstack/react-query";
import { api, errorMessage, meKey, type User } from "./api";
import { clearUserData, queryClient } from "./query";

export type Role = "super_admin" | "admin" | "affiliate";

export function roleOf(user: User): Role {
  return user.role === "affiliate" ? "affiliate" : user.role === "super_admin" ? "super_admin" : "admin";
}

export const isAdmin = (role: Role) => role !== "affiliate";

/** The signed-in user, or null when signed out. Errors only when the API is unreachable. */
export function useMe() {
  return useQuery({
    queryKey: meKey,
    queryFn: async ({ signal }): Promise<User | null> => {
      const { data } = await api.GET("/api/v1/auth/me", { signal });
      return data ?? null;
    },
    staleTime: 5 * 60_000,
  });
}

export function useLogin() {
  return useMutation({
    mutationFn: async (body: { email: string; password: string }) => {
      const { data, error } = await api.POST("/api/v1/auth/login", { body });
      if (!data) throw new Error(errorMessage(error));
      return data;
    },
    onSuccess: (user) => {
      clearUserData(meKey);
      queryClient.setQueryData(meKey, user);
    },
  });
}

export function useLogout() {
  return useMutation({
    mutationFn: () => api.POST("/api/v1/auth/logout"),
    onSettled: () => {
      clearUserData(meKey);
      queryClient.setQueryData(meKey, null);
    },
  });
}
