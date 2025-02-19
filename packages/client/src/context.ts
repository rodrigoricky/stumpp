import { JobUpdate } from '@stump/types'
import { QueryClient } from '@tanstack/react-query'
import { createContext, useContext } from 'react'

export const AppPropsContext = createContext<AppProps | null>(null)
export const QueryClientContext = createContext<QueryClient | undefined>(undefined)

export type StumpClientContextProps = {
	onRedirect?: (url: string) => void
}
export const StumpClientContext = createContext<StumpClientContextProps | undefined>(undefined)

export type Platform = 'browser' | 'macOS' | 'windows' | 'linux' | 'unknown'

export interface AppProps {
	platform: Platform
	baseUrl?: string
	demoMode?: boolean

	setBaseUrl?: (baseUrl: string) => void
	setUseDiscordPresence?: (connect: boolean) => void
	setDiscordPresence?: (status?: string, details?: string) => void
}

export interface IJobContext {
	activeJobs: Record<string, JobUpdate>

	addJob(job: JobUpdate): void
	updateJob(job: JobUpdate): void
	removeJob(runnerId: string): void
}
export const JobContext = createContext<IJobContext | null>(null)
export const useAppProps = () => useContext(AppPropsContext)
export const useJobContext = () => useContext(JobContext)
export const useClientContext = () => useContext(StumpClientContext)
