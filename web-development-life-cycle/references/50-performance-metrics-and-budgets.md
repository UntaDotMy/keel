# Web Performance Metrics and Budgets

## Core Web Vitals

Google's Core Web Vitals are the essential metrics for measuring user experience:

### 1. Largest Contentful Paint (LCP)
**Target: < 2.5 seconds**

Measures loading performance - when the largest content element becomes visible.

**What counts as LCP:**
- `<img>` elements
- `<image>` elements inside `<svg>`
- `<video>` elements (poster image)
- Background images loaded via CSS
- Block-level elements containing text

**How to improve:**
- Optimize images (WebP/AVIF, responsive images, lazy loading)
- Preload critical resources
- Minimize render-blocking resources
- Use CDN for static assets
- Implement server-side rendering or static generation
- Optimize server response time (TTFB < 600ms)

### 2. Interaction to Next Paint (INP)
**Target: ≤ 200ms** (good, 75th percentile of page loads)

Current Core Web Vital for interactivity (stable). Measures how quickly the page responds to user input across the full lifetime of a page, not only the first interaction.

**Historical note:** First Input Delay (FID) was the prior CWV for interactivity. INP replaced FID as a Core Web Vital in 2024 ([web.dev/blog/inp-cwv-launch](https://web.dev/blog/inp-cwv-launch)). Do not treat FID as a current CWV target; use INP in field tools, RUM, and release gates.

**How to improve:**
- Break up long JavaScript tasks (< 50ms each)
- Use web workers for heavy computation
- Defer non-critical JavaScript
- Code splitting and lazy loading
- Minimize third-party script impact
- Use `requestIdleCallback` for non-urgent work
- In the lab, Total Blocking Time (TBT) is a useful proxy when no real user input exists

### 3. Cumulative Layout Shift (CLS)
**Target: < 0.1**

Measures visual stability - unexpected layout shifts during page load.

**Common causes:**
- Images without dimensions
- Ads, embeds, iframes without reserved space
- Dynamically injected content
- Web fonts causing FOIT/FOUT
- Actions waiting for network response

**How to improve:**
- Always specify width and height on images and videos
- Reserve space for ads and embeds
- Use `font-display: swap` for web fonts
- Preload critical fonts
- Avoid inserting content above existing content
- Use CSS transforms for animations (not top/left)

## Performance Budgets

### JavaScript Budget
- **Initial Bundle**: < 200KB (gzipped)
- **Total JavaScript**: < 500KB (gzipped)
- **Third-party Scripts**: < 100KB (gzipped)

**Why it matters:**
- 200KB takes ~2s to parse/compile on mid-range mobile
- Every 100KB adds ~1s to Time to Interactive

**How to achieve:**
- Code splitting by route
- Lazy load non-critical features
- Tree shaking unused code
- Minimize dependencies
- Use lighter alternatives (date-fns instead of moment.js)

### Image Budget
- **Hero Images**: < 200KB (optimized)
- **Thumbnails**: < 50KB each
- **Total Images (initial load)**: < 1MB

**Optimization techniques:**
- Use WebP or AVIF when they reduce size without harming required visual quality
- Responsive images with `srcset`
- Lazy load below-the-fold images
- Use appropriate dimensions (don't serve 4K for 400px display)
- Compress with tools like ImageOptim, Squoosh

### CSS Budget
- **Critical CSS**: < 14KB (inline)
- **Total CSS**: < 100KB (gzipped)

**How to achieve:**
- Extract critical CSS for above-the-fold content
- Defer non-critical CSS
- Remove unused CSS (PurgeCSS, UnCSS)
- Use CSS-in-JS with code splitting (if applicable)

### Font Budget
- **Total Fonts**: < 100KB
- **Font Files**: 2-3 weights maximum

**Optimization:**
- Use variable fonts (single file, multiple weights)
- Subset fonts (only include needed characters)
- Preload critical fonts
- Use `font-display: swap`
- Consider system fonts for body text

## Measurement Tools

### Lighthouse
**What it measures:**
- Performance score (0-100)
- Core Web Vitals
- Accessibility
- Best practices
- SEO

**How to use:**
- Chrome DevTools > Lighthouse tab
- Run in incognito mode
- Test on mobile and desktop
- Run multiple times (results vary)

**CI Integration:**
```bash
npm install -g @lhci/cli
lhci autorun --collect.url=https://example.com
```

### WebPageTest
**What it provides:**
- Detailed waterfall charts
- Filmstrip view
- Multiple locations and devices
- Connection throttling
- Repeat views (cached)

**How to use:**
- Visit webpagetest.org
- Enter URL and select test location
- Choose device and connection speed
- Analyze waterfall and filmstrip

### Chrome DevTools Performance Panel
**What it shows:**
- Frame rate (FPS)
- CPU usage
- Network activity
- Long tasks (> 50ms)
- Layout shifts

**How to use:**
- Open DevTools > Performance tab
- Record page load or interaction
- Analyze flame chart for bottlenecks
- Identify long tasks to optimize

### Real User Monitoring (RUM)
**Tools:**
- Google Analytics 4 (Core Web Vitals)
- Vercel Analytics
- Cloudflare Web Analytics
- Custom implementation with `web-vitals` library

**Why it matters:**
- Lab data (Lighthouse) doesn't reflect real user experience
- RUM shows actual performance across devices/networks
- Identifies performance issues in production

## Optimization Techniques

### 1. Critical Rendering Path
**Goal:** Minimize time to first render

**Techniques:**
- Inline critical CSS (above-the-fold styles)
- Defer non-critical CSS
- Minimize render-blocking JavaScript
- Preload critical resources
- Use resource hints (preconnect, dns-prefetch)

### 2. Code Splitting
**Goal:** Load only what's needed

**Techniques:**
- Route-based splitting (one bundle per page)
- Component-based splitting (lazy load heavy components)
- Vendor splitting (separate bundle for dependencies)
- Dynamic imports (`import()`)

**Example (React):**
```javascript
const HeavyComponent = lazy(() => import('./HeavyComponent'));

function App() {
  return (
    <Suspense fallback={<Loading />}>
      <HeavyComponent />
    </Suspense>
  );
}
```

### 3. Image Optimization
**Techniques:**
- Use modern formats (WebP, AVIF)
- Responsive images with `srcset`
- Lazy loading with `loading="lazy"`
- Blur-up placeholder technique
- Use CDN with automatic optimization (Cloudinary, Imgix)

**Example:**
```html
<img
  src="image-800.webp"
  srcset="image-400.webp 400w, image-800.webp 800w, image-1200.webp 1200w"
  sizes="(max-width: 600px) 400px, (max-width: 1200px) 800px, 1200px"
  alt="Description"
  loading="lazy"
  width="800"
  height="600"
/>
```

### 4. Caching Strategy
**Techniques:**
- Browser caching (Cache-Control headers)
- Service workers (offline support, cache-first strategy)
- CDN caching (edge caching for static assets)
- API response caching (Redis, in-memory)

**Cache-Control examples:**
```
# Static assets (immutable)
Cache-Control: public, max-age=31536000, immutable

# HTML (revalidate)
Cache-Control: public, max-age=0, must-revalidate

# API responses (short-lived)
Cache-Control: public, max-age=300
```

### 5. Compression
**Techniques:**
- Gzip (widely supported, ~70% reduction)
- Brotli (better compression, ~80% reduction)
- Minification (remove whitespace, comments)

**Server configuration (Nginx):**
```nginx
gzip on;
gzip_types text/plain text/css application/json application/javascript;
brotli on;
brotli_types text/plain text/css application/json application/javascript;
```

## Performance Monitoring

### Continuous Monitoring
- Set up Lighthouse CI in your deployment pipeline
- Monitor Core Web Vitals in production (RUM)
- Set up alerts for performance regressions
- Track performance metrics over time

### Performance Budget Enforcement
```json
{
  "budgets": [
    {
      "resourceSizes": [
        { "resourceType": "script", "budget": 200 },
        { "resourceType": "image", "budget": 1000 },
        { "resourceType": "stylesheet", "budget": 100 }
      ]
    }
  ]
}
```

### Regression Detection
- Run Lighthouse on every PR
- Compare metrics against baseline
- Fail CI if performance degrades significantly
- Track bundle size changes

## Common Performance Pitfalls

1. **Not optimizing images** - Largest impact on LCP
2. **Blocking JavaScript** - Delays interactivity
3. **Missing dimensions on images** - Causes layout shifts
4. **Large bundle sizes** - Slow parse/compile time
5. **Not using CDN** - Slow asset delivery
6. **Synchronous third-party scripts** - Blocks rendering
7. **Not measuring real user performance** - Lab data doesn't reflect reality
8. **Premature optimization** - Optimize based on measurements, not assumptions

## Resources

- [Web.dev Performance](https://web.dev/performance/)
- [Core Web Vitals](https://web.dev/vitals/)
- [Lighthouse Documentation](https://developers.google.com/web/tools/lighthouse)
- [WebPageTest](https://www.webpagetest.org/)
- [web-vitals library](https://github.com/GoogleChrome/web-vitals)
