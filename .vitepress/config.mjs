import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'sepia',
  description: 'Agent sessions, agnostically — canonical IR, store drivers, ACP client.',
  base: '/sepia/',
  themeConfig: {
    nav: [
      { text: 'Tutorials', link: '/tutorials/getting-started' },
      { text: 'How-to', link: '/how-to/' },
      { text: 'Reference', link: '/reference/protocol' },
      { text: 'Explanation', link: '/explanation/' },
    ],
    sidebar: [
      {
        text: 'Tutorials',
        items: [{ text: 'Getting started', link: '/tutorials/getting-started' }],
      },
      {
        text: 'How-to',
        items: [
          { text: 'Convert a session', link: '/how-to/convert-a-session' },
          { text: 'Write a store driver', link: '/how-to/write-a-driver' },
          { text: 'Drive sepia serve', link: '/how-to/drive-serve' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'Driver protocol', link: '/reference/protocol' },
          { text: 'Serve wire', link: '/reference/serve-wire' },
          { text: 'Session formats', link: '/reference/session-formats' },
          { text: 'Agent configs', link: '/reference/agent-configs' },
        ],
      },
      {
        text: 'Explanation',
        items: [{ text: 'Architecture', link: '/explanation/architecture' }],
      },
    ],
    socialLinks: [{ icon: 'github', link: 'https://github.com/9aia/sepia' }],
  },
})
