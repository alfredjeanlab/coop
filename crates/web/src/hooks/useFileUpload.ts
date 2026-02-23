// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { useEffect, useRef, useState } from "react";
import { apiPost } from "@/hooks/useApiClient";
import { readFileAsBase64 } from "@/lib/base64";

interface UseFileUploadOptions {
  /** API path for upload (e.g. "/api/v1/upload" or per-session) */
  uploadPath: string | (() => string | null);
  /** Called with uploaded file paths */
  onUploaded?: (paths: string[]) => void;
  /** Called on error */
  onError?: (msg: string) => void;
}

export function useFileUpload({ uploadPath, onUploaded, onError }: UseFileUploadOptions) {
  const [dragActive, setDragActive] = useState(false);
  const dragCounterRef = useRef(0);

  function getPath() {
    return typeof uploadPath === "function" ? uploadPath() : uploadPath;
  }

  async function uploadFiles(files: FileList) {
    const path = getPath();
    if (!path) {
      onError?.("No upload path available");
      return;
    }

    const paths: string[] = [];
    for (const file of Array.from(files)) {
      try {
        const data = await readFileAsBase64(file);
        const res = await apiPost(path, { filename: file.name, data });
        if (res.ok && (res.json as { path?: string })?.path) {
          paths.push((res.json as { path: string }).path);
        } else {
          const msg =
            (res.json as { error?: { message?: string } })?.error?.message ||
            res.text ||
            "unknown error";
          onError?.(`upload error: ${msg}`);
        }
      } catch (err) {
        onError?.(`upload error: ${err instanceof Error ? err.message : String(err)}`);
      }
    }

    if (paths.length) {
      onUploaded?.(paths);
    }
  }

  const uploadFilesRef = useRef(uploadFiles);
  uploadFilesRef.current = uploadFiles;

  useEffect(() => {
    const onDragEnter = (e: DragEvent) => {
      e.preventDefault();
      dragCounterRef.current++;
      if (dragCounterRef.current === 1) setDragActive(true);
    };

    const onDragLeave = (e: DragEvent) => {
      e.preventDefault();
      dragCounterRef.current--;
      if (dragCounterRef.current <= 0) {
        dragCounterRef.current = 0;
        setDragActive(false);
      }
    };

    const onDragOver = (e: DragEvent) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    };

    const onDrop = (e: DragEvent) => {
      e.preventDefault();
      dragCounterRef.current = 0;
      setDragActive(false);
      if (e.dataTransfer?.files.length) {
        uploadFilesRef.current(e.dataTransfer.files);
      }
    };

    document.addEventListener("dragenter", onDragEnter);
    document.addEventListener("dragleave", onDragLeave);
    document.addEventListener("dragover", onDragOver);
    document.addEventListener("drop", onDrop);

    return () => {
      document.removeEventListener("dragenter", onDragEnter);
      document.removeEventListener("dragleave", onDragLeave);
      document.removeEventListener("dragover", onDragOver);
      document.removeEventListener("drop", onDrop);
    };
  }, []);

  return { dragActive };
}
