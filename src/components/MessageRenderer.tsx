import { MessageBody } from "../lib/markdown";

interface MessageRendererProps {
  content: string;
}

export function MessageRenderer({ content }: MessageRendererProps) {
  return <MessageBody content={content} />;
}
