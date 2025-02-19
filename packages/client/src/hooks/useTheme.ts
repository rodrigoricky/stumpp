import { useUpdatePreferences, useUserStore } from '..'

export type UseThemeParams = {
	onError?: (err: unknown) => void
}

/** A hook to get the current theme and toggle it. Please note that this hook
 *  **will not** handle the class toggling required by tailwind. That logic
 *  is handled by the `interface` package.
 **/
export function useTheme({ onError }: UseThemeParams = {}) {
	const { userPreferences, setUserPreferences } = useUserStore((state) => ({
		setUserPreferences: state.setUserPreferences,
		user: state.user,
		userPreferences: state.userPreferences,
	}))
	const { update } = useUpdatePreferences()

	const isDark = (userPreferences?.app_theme || 'light').toLowerCase() === 'dark'

	async function toggleTheme() {
		if (userPreferences) {
			const newTheme = isDark ? 'light' : 'dark'
			setUserPreferences({
				...userPreferences,
				app_theme: newTheme,
			})

			try {
				await update({
					...userPreferences,
					app_theme: newTheme,
				})
			} catch (err) {
				console.error(err)
				onError?.(err)
			}
		}
	}

	return { isDark, theme: userPreferences?.app_theme || 'light', toggleTheme }
}
