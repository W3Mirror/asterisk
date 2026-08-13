import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Asterisk Portal",
  description: "Management portal for the Asterisk PBX",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
