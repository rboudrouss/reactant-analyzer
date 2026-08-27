import { Sidebar } from "@/components/sidebar";

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html>
      <body>
        <Sidebar />
        {children}
      </body>
    </html>
  );
}
