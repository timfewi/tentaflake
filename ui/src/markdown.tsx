import Markdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const components: Components = {
  // Content from an agent or a workspace must not make the browser fetch anything.
  img: ({ alt }) => <span className="md-image">[image: {alt || "no description"}]</span>,
  a: ({ href, children }) =>
    href ? (
      <a href={href} target="_blank" rel="noopener noreferrer nofollow">
        {children}
      </a>
    ) : (
      <span>{children}</span>
    ),
};

/**
 * Markdown from an agent or a workspace, rendered as untrusted text: raw HTML
 * is dropped, images are not loaded and only http(s) links are clickable.
 */
export function SafeMarkdown({ body }: { body: string }) {
  return (
    <div className="markdown">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={components}
        skipHtml
        urlTransform={(url) => (/^https?:\/\//i.test(url) ? url : "")}
      >
        {body}
      </Markdown>
    </div>
  );
}
