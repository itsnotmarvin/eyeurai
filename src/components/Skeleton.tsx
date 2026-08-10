/** Loading placeholder that mirrors the account card rhythm. */
export function SkeletonList({ count = 3 }: { count?: number }) {
  return (
    <div className="skeletons" role="status" aria-label="Loading quotas">
      {Array.from({ length: count }, (_, index) => (
        <div className="skeleton" key={index} style={{ animationDelay: `${index * 90}ms` }}>
          <div className="skeleton__head">
            <span className="skeleton__pill skeleton__pill--wide" />
            <span className="skeleton__pill skeleton__pill--tiny" />
          </div>
          <span className="skeleton__bar" />
          <span className="skeleton__bar skeleton__bar--short" />
        </div>
      ))}
    </div>
  );
}
