import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Produces .next/standalone with a minimal server.js + only the
  // node_modules actually needed at runtime — keeps the docker image slim.
  output: "standalone",
  eslint: {
    // Linting is run separately in CI; don't let it block production builds.
    ignoreDuringBuilds: true,
  },
};

export default nextConfig;
