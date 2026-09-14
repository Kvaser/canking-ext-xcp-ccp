pipeline {
    agent { label 'ubuntu' }

    triggers {
        cron('H H/8 * * *')
    }

    environment {
        GITEA_SERVER    = 'http://git.kvaser.se'
        GITEA_SSH_HOST  = 'git.kvaser.se'
        GITEA_OWNER     = 'kvaser'
        GITEA_REPO      = 'canking-ext-xcp-ccp'
        GITHUB_OWNER    = 'Kvaser'
        GITHUB_REPO     = 'canking-ext-xcp-ccp'
        BASE_BRANCH     = 'develop'
    }

    stages {
        stage('Set up SSH auth') {
            steps {
                withCredentials([sshUserPrivateKey(credentialsId: 'pr-sync-ssh-key', keyFileVariable: 'SSH_KEY')]) {
                    sh '''
                        mkdir -p ~/.ssh
                        cp "$SSH_KEY" ~/.ssh/id_ed25519
                        chmod 600 ~/.ssh/id_ed25519
                        ssh-keyscan -H "${GITEA_SSH_HOST}" >> ~/.ssh/known_hosts 2>/dev/null
                    '''
                }
            }
        }

        stage('Checkout') {
            steps {
                sh '''
                    git init -q .
                    git remote add origin "git@${GITEA_SSH_HOST}:${GITEA_OWNER}/${GITEA_REPO}.git"
                    git fetch --prune origin "+refs/heads/*:refs/remotes/origin/*"
                    git checkout -q -B "${BASE_BRANCH}" "origin/${BASE_BRANCH}"
                '''
            }
        }

        stage('Fetch PR refs from GitHub') {
            steps {
                withCredentials([string(credentialsId: 'gh-api-token', variable: 'GH_API_TOKEN')]) {
                    sh '''
                        auth=$(printf "x-access-token:%s" "$GH_API_TOKEN" | base64 -w0)
                        git remote add github "https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}.git"
                        git -c http.extraheader="Authorization: Basic ${auth}" \
                          fetch github "+refs/pull/*/head:refs/remotes/github/pr/*" --prune
                    '''
                }
            }
        }

        stage('Sync PRs into Gitea') {
            steps {
                withCredentials([
                    string(credentialsId: 'pr-sync-token', variable: 'GITEA_TOKEN'),
                    string(credentialsId: 'gh-api-token', variable: 'GITHUB_TOKEN')
                ]) {
                    sh 'python3 jenkins/scripts/sync_github_prs.py'
                }
            }
        }
    }
}
